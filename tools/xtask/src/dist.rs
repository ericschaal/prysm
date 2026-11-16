//! `cargo xtask dist`: artifacts under fixed names in `dist/`.
use crate::manifest::{Entry, Manifest};
use crate::npm::{self, Script};
use crate::process::{self, root, target_dir};
use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// What `dist` can build. A new target is one variant here plus one arm in
/// [`Target::file_name`] and [`Target::build`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// prysm-headless for this Mac (macOS only).
    Mac,
    /// prysm-headless for 64-bit ARM Linux, built in Docker.
    LinuxArm64,
    /// prysm-headless for x86-64 Linux, built in Docker.
    LinuxAmd64,
    /// The Debian package for 64-bit ARM, built in Docker.
    DebArm64,
    /// The Debian package for x86-64, built in Docker.
    DebAmd64,
}

/// A CPU architecture, named as Debian and Docker name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    Amd64,
}

impl Arch {
    pub fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::Amd64 => "amd64",
        }
    }

    /// The architecture xtask was built for, which is the build machine's.
    fn host() -> Result<Self> {
        match std::env::consts::ARCH {
            "aarch64" => Ok(Self::Arm64),
            "x86_64" => Ok(Self::Amd64),
            other => bail!("no artifact name for the {other} architecture"),
        }
    }
}

impl Target {
    /// The artifact's name in `dist/`. `host` names the Mac binary.
    fn file_name(self, host: Arch) -> String {
        match self {
            Self::Mac => format!("prysm-headless-macos-{}", host.name()),
            Self::LinuxArm64 | Self::LinuxAmd64 => {
                format!("prysm-headless-linux-{}", self.arch(host).name())
            }
            Self::DebArm64 | Self::DebAmd64 => {
                format!("prysm-headless-linux-{}.deb", self.arch(host).name())
            }
        }
    }

    fn arch(self, host: Arch) -> Arch {
        match self {
            Self::Mac => host,
            Self::LinuxArm64 | Self::DebArm64 => Arch::Arm64,
            Self::LinuxAmd64 | Self::DebAmd64 => Arch::Amd64,
        }
    }

    /// Builds the artifact and copies it to `out`.
    fn build(self, out: &Path, host: Arch) -> Result<()> {
        match self {
            Self::Mac => mac(out),
            Self::LinuxArm64 | Self::LinuxAmd64 => binary(self.arch(host), out),
            Self::DebArm64 | Self::DebAmd64 => deb(self.arch(host), out),
        }
    }
}

/// Builds each target in turn and records it in `dist/manifest.json` as soon
/// as it is done, so an earlier success is kept when a later target fails.
pub fn run(targets: &[Target]) -> Result<()> {
    let host = Arch::host()?;
    let dist = root().join("dist");
    fs::create_dir_all(&dist).with_context(|| format!("creating {}", dist.display()))?;
    let source = Source::current()?;
    // Read first, so a broken manifest fails before a long build.
    let manifest_path = dist.join("manifest.json");
    let mut manifest = Manifest::read(&manifest_path)?;
    let mut done = Vec::new();
    for &target in targets {
        if done.contains(&target) {
            continue;
        }
        let name = target.file_name(host);
        let out = dist.join(&name);
        target
            .build(&out, host)
            .with_context(|| format!("building {name}"))?;
        let entry = source.entry(sha256(&out)?, SystemTime::now());
        manifest.insert(name.clone(), entry);
        manifest.write(&manifest_path)?;
        eprintln!("Wrote dist/{name}");
        done.push(target);
    }
    Ok(())
}

/// The bundle, then a release build of prysm-headless for this Mac.
fn mac(out: &Path) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "the mac target builds only on macOS"
    );
    npm::run(Script::Build)?;
    process::run(&mut process::command(
        "cargo",
        &["build", "--release", "--locked", "-p", "prysm-headless"],
    ))?;
    copy(&target_dir().join("release/prysm-headless"), out)
}

/// The Dockerfile's `binary` target, which exports `/prysm-headless`.
fn binary(arch: Arch, out: &Path) -> Result<()> {
    let staging = docker_export(arch, DockerStage::Binary, out)?;
    copy(&staging.join("prysm-headless"), out)
}

/// The Dockerfile's `deb` target, which exports one versioned `.deb`.
fn deb(arch: Arch, out: &Path) -> Result<()> {
    let staging = docker_export(arch, DockerStage::Deb, out)?;
    let suffix = format!("_{}.deb", arch.name());
    let mut debs = Vec::new();
    for entry in fs::read_dir(&staging).with_context(|| format!("reading {}", staging.display()))? {
        let path = entry?.path();
        if path.to_string_lossy().ends_with(&suffix) {
            debs.push(path);
        }
    }
    let [package] = debs.as_slice() else {
        bail!(
            "expected one *{suffix} in {}, found {debs:?}",
            staging.display()
        );
    };
    copy(package, out)
}

/// A stage of `packaging/docker/Dockerfile` that exports files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockerStage {
    Binary,
    Deb,
}

impl DockerStage {
    fn name(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Deb => "deb",
        }
    }
}

/// Builds `stage` for `arch` and exports its files into an emptied
/// `target/xtask/<artifact name>`, which it returns.
fn docker_export(arch: Arch, stage: DockerStage, out: &Path) -> Result<PathBuf> {
    let name = out.file_name().context("an artifact path names a file")?;
    let staging = target_dir().join("xtask").join(name);
    empty_dir(&staging)?;
    process::run(&mut docker_build(arch, stage, &staging))?;
    Ok(staging)
}

/// `docker buildx build` of `stage` for `arch`, exported into `dest`.
fn docker_build(arch: Arch, stage: DockerStage, dest: &Path) -> Command {
    let platform = format!("linux/{}", arch.name());
    let output = format!("type=local,dest={}", dest.display());
    process::command(
        "docker",
        &[
            "buildx",
            "build",
            "--file",
            "packaging/docker/Dockerfile",
            "--platform",
            &platform,
            "--target",
            stage.name(),
            "--output",
            &output,
            ".",
        ],
    )
}

/// The checkout an artifact is built from.
struct Source {
    commit: String,
    dirty: bool,
}

impl Source {
    fn current() -> Result<Self> {
        let git = |args: &[&str]| process::output(&mut process::command("git", args));
        Ok(Self {
            commit: git(&["rev-parse", "HEAD"]).context("reading the commit")?,
            dirty: !git(&["status", "--porcelain"])
                .context("reading the tree's status")?
                .is_empty(),
        })
    }

    fn entry(&self, sha256: String, built: SystemTime) -> Entry {
        Entry {
            version: prysm_version::VERSION.to_owned(),
            commit: self.commit.clone(),
            dirty: self.dirty,
            sha256,
            built_at: humantime::format_rfc3339_seconds(built).to_string(),
        }
    }
}

fn sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Copies `from` to `to`, keeping the permissions. The copy goes to a
/// temporary name first: macOS may kill a signed binary rewritten in place.
fn copy(from: &Path, to: &Path) -> Result<()> {
    let mut temporary = to.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::copy(from, &temporary)
        .and_then(|_| fs::rename(&temporary, to))
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))
}

/// Creates `dir`, removing what an earlier run left in it.
fn empty_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("removing {}", dir.display()));
        }
        _ => {}
    }
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn artifacts_have_fixed_names() {
        let names = Target::value_variants()
            .iter()
            .map(|target| target.file_name(Arch::Arm64))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "prysm-headless-macos-arm64",
                "prysm-headless-linux-arm64",
                "prysm-headless-linux-amd64",
                "prysm-headless-linux-arm64.deb",
                "prysm-headless-linux-amd64.deb",
            ]
        );
        assert_eq!(
            Target::Mac.file_name(Arch::Amd64),
            "prysm-headless-macos-amd64"
        );
    }

    #[test]
    fn target_names_on_the_command_line() {
        let names = Target::value_variants()
            .iter()
            .map(|target| target.to_possible_value().unwrap().get_name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "mac",
                "linux-arm64",
                "linux-amd64",
                "deb-arm64",
                "deb-amd64"
            ]
        );
    }

    #[test]
    fn docker_builds_one_stage_for_one_platform() {
        let command = docker_build(Arch::Amd64, DockerStage::Deb, Path::new("/tmp/out"));
        assert_eq!(
            process::display(&command),
            "docker buildx build --file packaging/docker/Dockerfile --platform linux/amd64 \
             --target deb --output type=local,dest=/tmp/out ."
        );
        assert_eq!(command.get_current_dir(), Some(root()));
    }

    #[test]
    fn entries_carry_the_product_version_and_utc_time() {
        let source = Source {
            commit: "abc".into(),
            dirty: true,
        };
        let built = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let entry = source.entry("00".into(), built);
        assert_eq!(entry.version, prysm_version::VERSION);
        assert_eq!(entry.built_at, "2023-11-14T22:13:20Z");
        assert!(entry.dirty);
    }
}
