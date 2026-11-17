//! `cargo xtask doctor`: which tools are installed and what needs them.
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The pinned toolchain file, which rustup reads in the repository.
const RUST_TOOLCHAIN: &str = include_str!("../../../rust-toolchain.toml");

/// The Rust version `rust-toolchain.toml` pins, such as `1.99.1`.
fn pinned_rust() -> &'static str {
    RUST_TOOLCHAIN
        .lines()
        .find_map(|line| line.strip_prefix("channel = "))
        .map(|value| value.trim_matches('"'))
        .expect("rust-toolchain.toml names its channel")
}

/// A tool a command needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Cargo,
    Rustc,
    Node,
    Npm,
    Python,
    /// Runs the acceptance runner's checks in its locked environment.
    Uv,
    Git,
    Docker,
    Buildx,
    /// bindgen needs clang and libclang on Linux (V4L2 bindings).
    Clang,
    Libclang,
    Ffmpeg,
    /// The relocated webOS SDK in `WEBOS_SDK`, read through its gcc.
    WebosSdk,
    AresPackage,
}

/// What a tool is needed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    Check,
    Codegen,
    DistMac,
    DistLinux,
    DistTv,
    /// The `file` source and its tests; they skip without FFmpeg.
    FileSource,
}

impl fmt::Display for Use {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Check => "check",
            Self::Codegen => "codegen",
            Self::DistMac => "dist mac",
            Self::DistLinux => "dist linux-*/deb-*/image-*",
            Self::DistTv => "dist tv-*",
            Self::FileSource => "the file source only",
        })
    }
}

/// A `major.minor` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32);

impl Tool {
    const ALL: [Self; 14] = [
        Self::Cargo,
        Self::Rustc,
        Self::Node,
        Self::Npm,
        Self::Python,
        Self::Uv,
        Self::Git,
        Self::Docker,
        Self::Buildx,
        Self::Clang,
        Self::Libclang,
        Self::Ffmpeg,
        Self::WebosSdk,
        Self::AresPackage,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Rustc => "rustc",
            Self::Node => "node",
            Self::Npm => "npm",
            Self::Python => "python3",
            Self::Uv => "uv",
            Self::Git => "git",
            Self::Docker => "docker",
            Self::Buildx => "buildx",
            Self::Clang => "clang",
            Self::Libclang => "libclang",
            Self::Ffmpeg => "ffmpeg",
            Self::WebosSdk => "webos-sdk",
            Self::AresPackage => "ares-package",
        }
    }

    /// The command that prints the version.
    fn version_command(self) -> Command {
        let (program, args): (PathBuf, &[&str]) = match self {
            Self::Buildx => ("docker".into(), &["buildx", "version"]),
            Self::Ffmpeg => ("ffmpeg".into(), &["-version"]),
            // Unset, the empty program fails to start: missing.
            Self::WebosSdk => (
                std::env::var_os("WEBOS_SDK")
                    .map(|sdk| Path::new(&sdk).join("bin/arm-webos-linux-gnueabi-gcc"))
                    .unwrap_or_default(),
                &["--version"],
            ),
            other => (other.name().into(), &["--version"]),
        };
        let mut command = Command::new(program);
        command.args(args);
        command
    }

    /// The oldest version that works, where one is known.
    fn minimum(self) -> Option<Version> {
        match self {
            // In the repository, rustup runs exactly this one.
            Self::Cargo | Self::Rustc => parse_version(pinned_rust()).map(|(version, _)| version),
            Self::Node => Some(Version(24, 0)),
            Self::Python => Some(Version(3, 9)),
            Self::Ffmpeg => Some(Version(9, 0)),
            _ => None,
        }
    }

    fn uses(self) -> &'static [Use] {
        match self {
            Self::Cargo | Self::Rustc | Self::Node | Self::Npm => {
                &[Use::Check, Use::Codegen, Use::DistMac, Use::DistTv]
            }
            Self::Python => &[Use::Check, Use::DistTv],
            Self::Uv => &[Use::Check],
            Self::Git => &[Use::Check, Use::DistMac, Use::DistLinux, Use::DistTv],
            Self::Docker | Self::Buildx => &[Use::DistLinux],
            Self::Clang | Self::Libclang => &[Use::Check, Use::Codegen],
            Self::Ffmpeg => &[Use::FileSource],
            Self::WebosSdk | Self::AresPackage => &[Use::DistTv],
        }
    }

    /// Whether this machine needs the tool at all.
    fn applies(self) -> bool {
        !matches!(self, Self::Clang | Self::Libclang) || cfg!(target_os = "linux")
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Cargo | Self::Rustc => {
                "install rustup, which runs the Rust that rust-toolchain.toml pins"
            }
            Self::Node | Self::Npm => "install Node.js 24 or newer",
            Self::Python => "install Python 3.9 or newer",
            Self::Uv => "install uv, see docs/developers/acceptance.md#develop-the-runner",
            Self::Git => "install git",
            Self::Docker | Self::Buildx => "install Docker with the buildx plugin",
            Self::Clang => "install clang",
            Self::Libclang => "install libclang-dev, or set LIBCLANG_PATH to its directory",
            Self::Ffmpeg => "install FFmpeg 9 or newer",
            Self::WebosSdk => {
                "set WEBOS_SDK to the relocated SDK, see docs/developers/packaging.md#webos"
            }
            Self::AresPackage => "npm install --global @webos-tools/cli@3.2.6",
        }
    }
}

/// What was found for one tool.
#[derive(Debug, PartialEq, Eq)]
enum Status {
    /// Installed; the version as printed, if it could be read.
    Found(Option<String>),
    TooOld {
        found: String,
        minimum: Version,
    },
    Missing,
}

impl Status {
    fn of(tool: Tool) -> Self {
        if tool == Tool::Libclang {
            return libclang().map_or(Self::Missing, |name| Self::Found(Some(name)));
        }
        let Some(text) = tool
            .version_command()
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        else {
            return Self::Missing;
        };
        Self::judge(tool.minimum(), parse_version(&text))
    }

    fn judge(minimum: Option<Version>, found: Option<(Version, &str)>) -> Self {
        match (minimum, found) {
            (Some(minimum), Some((version, text))) if version < minimum => Self::TooOld {
                found: text.to_owned(),
                minimum,
            },
            (_, found) => Self::Found(found.map(|(_, text)| text.to_owned())),
        }
    }

    fn usable(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

/// The first word that looks like a version, such as `24.1.0` in
/// `v24.1.0`, `9.0.2` in `ffmpeg version n9.0.2` or `28.3.2` in
/// `Docker version 28.3.2, build …`.
fn parse_version(text: &str) -> Option<(Version, &str)> {
    text.split_whitespace().find_map(|word| {
        let word = word.trim_end_matches(',');
        let word = word.strip_prefix(['v', 'n']).unwrap_or(word);
        let mut parts = word.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor: String = parts
            .next()?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Some((Version(major, minor.parse().ok()?), word))
    })
}

/// The file name of a libclang that bindgen would load: in `LIBCLANG_PATH`
/// (a directory or the file) when set, else one the dynamic loader knows
/// (`ldconfig -p`), which is where Debian and Ubuntu install it.
fn libclang() -> Option<String> {
    let names: Vec<String> = match std::env::var_os("LIBCLANG_PATH") {
        Some(path) if Path::new(&path).is_dir() => fs::read_dir(path)
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
            .collect(),
        Some(path) => vec![Path::new(&path).file_name()?.to_string_lossy().into_owned()],
        None => {
            let cache = Command::new("/sbin/ldconfig").arg("-p").output().ok()?;
            String::from_utf8_lossy(&cache.stdout)
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_owned)
                .collect()
        }
    };
    names.into_iter().find(|name| is_libclang(name))
}

/// `libclang.so…` or `libclang-<version>.so…`, the names bindgen looks for;
/// not `libclang-cpp`, the C++ library.
fn is_libclang(name: &str) -> bool {
    name.starts_with("libclang.so")
        || name
            .strip_prefix("libclang-")
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Prints one line per tool; fails when `check` cannot run.
pub fn run() -> ExitCode {
    let mut check_blocked = false;
    for tool in Tool::ALL.into_iter().filter(|tool| tool.applies()) {
        let status = Status::of(tool);
        let uses = tool
            .uses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let (label, detail) = match &status {
            Status::Found(version) => ("ok", version.as_deref().unwrap_or("?").to_owned()),
            Status::TooOld { found, minimum } => {
                ("too old", format!("{found} < {}.{}", minimum.0, minimum.1))
            }
            Status::Missing => ("missing", "-".to_owned()),
        };
        let hint = if status.usable() {
            String::new()
        } else {
            format!(" ({})", tool.hint())
        };
        println!(
            "{label:<8} {:<12} {detail:<17} for {uses}{hint}",
            tool.name()
        );
        check_blocked |= !status.usable() && tool.uses().contains(&Use::Check);
    }
    if check_blocked {
        println!("`cargo xtask check` cannot run until the tools above are installed.");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_real_output() {
        let cases = [
            (
                "cargo 1.99.0 (5f94df478 2026-08-27)",
                Version(1, 99),
                "1.99.0",
            ),
            ("v26.13.0", Version(26, 13), "26.13.0"),
            ("11.6.2", Version(11, 6), "11.6.2"),
            ("Python 3.15.2", Version(3, 15), "3.15.2"),
            (
                "uv 0.13.5 (Homebrew 2026-10-22 aarch64-apple-darwin)",
                Version(0, 13),
                "0.13.5",
            ),
            (
                "Docker version 28.3.2, build 578ccf6",
                Version(28, 3),
                "28.3.2",
            ),
            (
                "github.com/docker/buildx v0.39.0 Homebrew",
                Version(0, 39),
                "0.39.0",
            ),
            (
                "ffmpeg version 9.0.2 Copyright (c) 2000-2026",
                Version(9, 0),
                "9.0.2",
            ),
            (
                "ffmpeg version n9.0-static https://…",
                Version(9, 0),
                "9.0-static",
            ),
            ("Debian clang version 14.0.6", Version(14, 0), "14.0.6"),
            ("Version: 3.2.6", Version(3, 2), "3.2.6"),
            (
                "arm-webos-linux-gnueabi-gcc.br_real (Buildroot -g) 12.2.0",
                Version(12, 2),
                "12.2.0",
            ),
        ];
        for (text, version, word) in cases {
            assert_eq!(parse_version(text), Some((version, word)), "{text}");
        }
    }

    #[test]
    fn a_git_build_has_no_version() {
        assert_eq!(parse_version("ffmpeg version N-118000-gabc123"), None);
    }

    #[test]
    fn old_versions_are_reported_and_unknown_ones_pass() {
        let old = Status::judge(Some(Version(24, 0)), parse_version("v22.3.0"));
        assert_eq!(
            old,
            Status::TooOld {
                found: "22.3.0".into(),
                minimum: Version(24, 0)
            }
        );
        assert!(Status::judge(Some(Version(9, 0)), None).usable());
        assert!(Status::judge(Some(Version(3, 9)), parse_version("Python 3.15.2")).usable());
    }

    #[test]
    fn the_dockerfile_builds_with_the_pinned_rust() {
        assert_eq!(Tool::Rustc.minimum().map(|version| version.0), Some(1));
        let dockerfile = include_str!("../../../packaging/docker/Dockerfile");
        let tag = format!("FROM rust:{}-", pinned_rust());
        assert!(
            dockerfile.contains(&tag),
            "packaging/docker/Dockerfile must build with `{tag}…`, as rust-toolchain.toml pins"
        );
    }

    #[test]
    fn libclang_is_the_c_library_in_any_version() {
        for name in [
            "libclang.so",
            "libclang.so.1",
            "libclang-18.so.18",
            "libclang-14.so",
        ] {
            assert!(is_libclang(name), "{name}");
        }
        for name in ["libclang-cpp.so.18", "libclangBasic.a", "libc.so.6"] {
            assert!(!is_libclang(name), "{name}");
        }
    }

    #[test]
    fn docker_is_not_needed_for_check() {
        assert!(!Tool::Docker.uses().contains(&Use::Check));
        assert!(!Tool::Ffmpeg.uses().contains(&Use::Check));
    }
}
