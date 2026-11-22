//! `cargo xtask doctor`: which tools are installed and what needs them.
use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use strum::{EnumIter, IntoEnumIterator};

/// The pinned toolchain file, which rustup reads in the repository.
const RUST_TOOLCHAIN: &str = include_str!("../../../../rust-toolchain.toml");

/// The Rust version `rust-toolchain.toml` pins, such as `1.99.1`.
fn pinned_rust() -> String {
    #[derive(Deserialize)]
    struct File {
        toolchain: Toolchain,
    }
    #[derive(Deserialize)]
    struct Toolchain {
        channel: String,
    }
    toml::from_str::<File>(RUST_TOOLCHAIN)
        .expect("rust-toolchain.toml names its channel")
        .toolchain
        .channel
}

/// A tool a command needs, reported in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter)]
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
pub(crate) enum Use {
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

/// A `major.minor.patch` version; a missing patch level is 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// What a probe found: what to show, and the version when there is one.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    shown: String,
    version: Option<Version>,
}

impl Found {
    /// The version in a tool's output, such as `24.1.0` from `v24.1.0`, or
    /// `?` when it has none.
    fn from_output(text: &str) -> Self {
        match parse_version(text) {
            Some((version, word)) => Self {
                shown: word.to_owned(),
                version: Some(version),
            },
            None => Self {
                shown: "?".to_owned(),
                version: None,
            },
        }
    }
}

impl Tool {
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

    /// What is installed, or `None` when the tool is missing.
    fn probe(self) -> Option<Found> {
        match self {
            // A library: its file name, which carries no version to compare.
            Self::Libclang => libclang().map(|name| Found {
                shown: name,
                version: None,
            }),
            _ => {
                let output = self.version_command().output().ok()?;
                output
                    .status
                    .success()
                    .then(|| Found::from_output(&String::from_utf8_lossy(&output.stdout)))
            }
        }
    }

    /// The command that prints the version, for every tool but libclang,
    /// which [`Tool::probe`] looks up as a file.
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
            Self::Cargo | Self::Rustc => parse_version(&pinned_rust()).map(|(version, _)| version),
            Self::Node => Some(Version(24, 0, 0)),
            Self::Python => Some(Version(3, 9, 0)),
            Self::Ffmpeg => Some(Version(9, 0, 0)),
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
    /// Installed; what the probe showed, `?` when it read no version.
    Found(String),
    TooOld {
        found: String,
        minimum: Version,
    },
    Missing,
}

impl Status {
    fn of(tool: Tool) -> Self {
        tool.probe()
            .map_or(Self::Missing, |found| Self::judge(tool.minimum(), found))
    }

    fn judge(minimum: Option<Version>, found: Found) -> Self {
        match (minimum, found.version) {
            (Some(minimum), Some(version)) if version < minimum => Self::TooOld {
                found: found.shown,
                minimum,
            },
            _ => Self::Found(found.shown),
        }
    }

    fn usable(&self) -> bool {
        matches!(self, Self::Found(_))
    }
}

/// The first word that looks like a version, such as `24.1.0` in
/// `v24.1.0`, `9.0.2` in `ffmpeg version n9.0.2` or `28.3.2` in
/// `Docker version 28.3.2, build …`. It needs a major and a minor number;
/// a missing or unreadable patch level is 0.
fn parse_version(text: &str) -> Option<(Version, &str)> {
    let number = |part: &str| -> Option<u32> {
        let digits = part
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(part.len());
        part[..digits].parse().ok()
    };
    text.split_whitespace().find_map(|word| {
        let word = word.trim_end_matches(',');
        let word = word.strip_prefix(['v', 'n']).unwrap_or(word);
        let mut parts = word.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = number(parts.next()?)?;
        let patch = parts.next().and_then(number).unwrap_or(0);
        Some((Version(major, minor, patch), word))
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

/// The tools this machine needs for `purpose`, in report order.
fn needed_for(purpose: Use) -> impl Iterator<Item = Tool> {
    Tool::iter().filter(move |tool| tool.applies() && tool.uses().contains(&purpose))
}

/// Fails naming every tool `purpose` needs that is missing or too old, with
/// its hint, as the report lists them.
pub(crate) fn require(purpose: Use) -> anyhow::Result<()> {
    let unusable = needed_for(purpose)
        .filter(|&tool| !Status::of(tool).usable())
        .map(|tool| format!("{} ({})", tool.name(), tool.hint()))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        unusable.is_empty(),
        "{purpose} needs {}; see `cargo xtask doctor`",
        unusable.join(", ")
    );
    Ok(())
}

/// Prints one line per tool; fails when `check` cannot run. A reader that
/// stops early (`cargo xtask doctor | head -2`) ends it quietly.
pub fn run() -> ExitCode {
    match report(&mut io::stdout().lock()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) if error.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("doctor: writing the report: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the report to `out`; whether `check` can run.
fn report(out: &mut impl Write) -> io::Result<bool> {
    let mut check_blocked = false;
    for tool in Tool::iter().filter(|tool| tool.applies()) {
        let status = Status::of(tool);
        let uses = tool
            .uses()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let (label, detail) = match &status {
            Status::Found(shown) => ("ok", shown.clone()),
            Status::TooOld { found, minimum } => ("too old", format!("{found} < {minimum}")),
            Status::Missing => ("missing", "-".to_owned()),
        };
        let hint = if status.usable() {
            String::new()
        } else {
            format!(" ({})", tool.hint())
        };
        writeln!(
            out,
            "{label:<8} {:<12} {detail:<17} for {uses}{hint}",
            tool.name()
        )?;
        check_blocked |= !status.usable() && tool.uses().contains(&Use::Check);
    }
    if check_blocked {
        writeln!(
            out,
            "`cargo xtask check` cannot run until the tools above are installed."
        )?;
    }
    out.flush()?;
    Ok(!check_blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_real_output() {
        let cases = [
            (
                "cargo 1.99.0 (5f94df478 2026-08-27)",
                Version(1, 99, 0),
                "1.99.0",
            ),
            ("v26.13.0", Version(26, 13, 0), "26.13.0"),
            ("11.6.2", Version(11, 6, 2), "11.6.2"),
            ("Python 3.15.2", Version(3, 15, 2), "3.15.2"),
            (
                "uv 0.13.5 (Homebrew 2026-10-22 aarch64-apple-darwin)",
                Version(0, 13, 5),
                "0.13.5",
            ),
            (
                "Docker version 28.3.2, build 578ccf6",
                Version(28, 3, 2),
                "28.3.2",
            ),
            (
                "github.com/docker/buildx v0.39.0 Homebrew",
                Version(0, 39, 0),
                "0.39.0",
            ),
            (
                "ffmpeg version 9.0.2 Copyright (c) 2000-2026",
                Version(9, 0, 2),
                "9.0.2",
            ),
            (
                "ffmpeg version n9.0-static https://…",
                Version(9, 0, 0),
                "9.0-static",
            ),
            ("Debian clang version 14.0.6", Version(14, 0, 6), "14.0.6"),
            ("Version: 3.2.6", Version(3, 2, 6), "3.2.6"),
            (
                "arm-webos-linux-gnueabi-gcc.br_real (Buildroot -g) 12.2.0",
                Version(12, 2, 0),
                "12.2.0",
            ),
            ("rustc 1.99.1-beta.2", Version(1, 99, 1), "1.99.1-beta.2"),
        ];
        for (text, version, word) in cases {
            assert_eq!(parse_version(text), Some((version, word)), "{text}");
        }
    }

    #[test]
    fn a_git_build_has_no_version() {
        assert_eq!(parse_version("ffmpeg version N-118000-gabc123"), None);
        assert_eq!(
            Found::from_output("ffmpeg version N-118000-gabc123"),
            Found {
                shown: "?".into(),
                version: None
            }
        );
    }

    #[test]
    fn old_versions_are_reported_and_unknown_ones_pass() {
        let old = Status::judge(Some(Version(24, 0, 0)), Found::from_output("v22.3.0"));
        assert_eq!(
            old,
            Status::TooOld {
                found: "22.3.0".into(),
                minimum: Version(24, 0, 0)
            }
        );
        let unknown = Status::judge(Some(Version(9, 0, 0)), Found::from_output("N-1-gabc"));
        assert_eq!(unknown, Status::Found("?".into()));
        let python = Status::judge(Some(Version(3, 9, 0)), Found::from_output("Python 3.15.2"));
        assert!(python.usable());
    }

    #[test]
    fn the_patch_level_of_the_pinned_rust_counts() {
        let pinned = Version(1, 99, 1);
        assert!(!Status::judge(Some(pinned), Found::from_output("rustc 1.99.0 (x)")).usable());
        assert!(Status::judge(Some(pinned), Found::from_output("rustc 1.99.1 (x)")).usable());
        assert!(Status::judge(Some(pinned), Found::from_output("rustc 1.100.0 (x)")).usable());
        assert_eq!(pinned.to_string(), "1.99.1");
    }

    #[test]
    fn a_library_shows_its_file_name() {
        let found = Found {
            shown: "libclang-18.so.18".into(),
            version: None,
        };
        assert_eq!(
            Status::judge(Tool::Libclang.minimum(), found),
            Status::Found("libclang-18.so.18".into())
        );
    }

    #[test]
    fn the_dockerfile_builds_with_the_pinned_rust() {
        assert_eq!(
            Tool::Rustc.minimum().map(|version| version.to_string()),
            Some(pinned_rust())
        );
        let dockerfile = include_str!("../../../../packaging/docker/Dockerfile");
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
    fn each_dist_target_needs_all_its_tools() {
        let tools = |purpose| needed_for(purpose).collect::<Vec<_>>();
        use Tool::*;
        assert_eq!(
            tools(Use::DistTv),
            [Cargo, Rustc, Node, Npm, Python, Git, WebosSdk, AresPackage]
        );
        assert_eq!(tools(Use::DistMac), [Cargo, Rustc, Node, Npm, Git]);
        assert_eq!(tools(Use::DistLinux), [Git, Docker, Buildx]);
    }

    #[test]
    fn docker_is_not_needed_for_check() {
        assert!(!Tool::Docker.uses().contains(&Use::Check));
        assert!(!Tool::Ffmpeg.uses().contains(&Use::Check));
    }
}
