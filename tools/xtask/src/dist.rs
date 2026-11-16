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
/// [`Target::kind`], and a new [`Kind`] one arm in [`Kind::file_name`] and
/// [`Builder::build`].
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
    /// The Docker image for 64-bit ARM, as a `docker load` archive.
    ImageArm64,
    /// The Docker image for x86-64, as a `docker load` archive.
    ImageAmd64,
    /// The full TV package; needs the webOS SDK in WEBOS_SDK.
    TvFull,
    /// The slim TV package; needs the webOS SDK in WEBOS_SDK.
    TvSlim,
}

/// What a target builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Mac,
    Binary(Arch),
    Deb(Arch),
    Image(Arch),
    Tv(Flavour),
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

/// A TV package, as `packaging/webos/build-ipk.sh` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flavour {
    Full,
    Slim,
}

impl Flavour {
    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Slim => "slim",
        }
    }
}

impl Target {
    fn kind(self) -> Kind {
        match self {
            Self::Mac => Kind::Mac,
            Self::LinuxArm64 => Kind::Binary(Arch::Arm64),
            Self::LinuxAmd64 => Kind::Binary(Arch::Amd64),
            Self::DebArm64 => Kind::Deb(Arch::Arm64),
            Self::DebAmd64 => Kind::Deb(Arch::Amd64),
            Self::ImageArm64 => Kind::Image(Arch::Arm64),
            Self::ImageAmd64 => Kind::Image(Arch::Amd64),
            Self::TvFull => Kind::Tv(Flavour::Full),
            Self::TvSlim => Kind::Tv(Flavour::Slim),
        }
    }
}

impl Kind {
    /// The artifact's name in `dist/`. `host` names the Mac binary. The
    /// Dockerfile's `artifacts` stage exports the binary and the `.deb`
    /// under these names too.
    fn file_name(self, host: Arch) -> String {
        match self {
            Self::Mac => format!("prysm-headless-macos-{}", host.name()),
            Self::Binary(arch) => format!("prysm-headless-linux-{}", arch.name()),
            Self::Deb(arch) => format!("prysm-headless-linux-{}.deb", arch.name()),
            Self::Image(arch) => format!("prysm-headless-linux-{}.tar", arch.name()),
            Self::Tv(flavour) => format!("prysm-tv-{}.ipk", flavour.name()),
        }
    }
}

/// Builds each target in turn and records it in `dist/manifest.json` as soon
/// as it is done, so an earlier success is kept when a later target fails.
pub fn run(targets: &[Target]) -> Result<()> {
    let dist = root().join("dist");
    fs::create_dir_all(&dist).with_context(|| format!("creating {}", dist.display()))?;
    let mut builder = Builder {
        host: Arch::host()?,
        source: Source::current()?,
        exported: Vec::new(),
    };
    // Read first, so a broken manifest fails before a long build.
    let manifest_path = dist.join("manifest.json");
    let mut manifest = Manifest::read(&manifest_path)?;
    let mut done = Vec::new();
    for &target in targets {
        if done.contains(&target) {
            continue;
        }
        let kind = target.kind();
        let name = kind.file_name(builder.host);
        let out = dist.join(&name);
        builder
            .build(kind, &out)
            .with_context(|| format!("building {name}"))?;
        let entry = builder.source.entry(sha256(&out)?, SystemTime::now());
        manifest.insert(name.clone(), entry);
        manifest.write(&manifest_path)?;
        eprintln!("Wrote dist/{name}");
        done.push(target);
    }
    Ok(())
}

/// State shared by the targets of one `dist` run.
struct Builder {
    host: Arch,
    source: Source,
    /// Architectures whose `artifacts` stage this run has exported.
    exported: Vec<Arch>,
}

impl Builder {
    /// Builds the artifact of `kind` and copies it to `out`.
    fn build(&mut self, kind: Kind, out: &Path) -> Result<()> {
        match kind {
            Kind::Mac => mac(out),
            Kind::Binary(arch) | Kind::Deb(arch) => {
                let exported = self.docker_artifacts(arch)?;
                copy(&exported.join(kind.file_name(self.host)), out)
            }
            Kind::Image(arch) => image(arch, &self.source.commit, out),
            Kind::Tv(flavour) => tv(flavour, out),
        }
    }

    /// Exports the Dockerfile's `artifacts` stage for `arch` (the binary and
    /// the `.deb`) into `target/xtask/docker-<arch>`, once per run.
    fn docker_artifacts(&mut self, arch: Arch) -> Result<PathBuf> {
        let dir = target_dir()
            .join("xtask")
            .join(format!("docker-{}", arch.name()));
        if !self.exported.contains(&arch) {
            empty_dir(&dir)?;
            let output = format!("type=local,dest={}", dir.display());
            process::run(&mut docker_build(
                arch,
                DockerStage::Artifacts,
                &output,
                &[],
            ))?;
            self.exported.push(arch);
        }
        Ok(dir)
    }
}

/// The bundle, then a release build of prysm-headless for this Mac.
fn mac(out: &Path) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "the mac target builds only on macOS"
    );
    npm::run(Script::Bundle)?;
    process::run(&mut process::command(
        "cargo",
        &["build", "--release", "--locked", "-p", "prysm-headless"],
    ))?;
    copy(&target_dir().join("release/prysm-headless"), out)
}

/// The image `docker load` restores as `prysm-headless:local`, the name
/// `packaging/release/publish.sh` expects, labelled with its commit.
const IMAGE: &str = "prysm-headless:local";

/// The Dockerfile's `runtime` stage, saved as an image archive.
fn image(arch: Arch, commit: &str, out: &Path) -> Result<()> {
    let name = out.file_name().context("an artifact path names a file")?;
    let archive = target_dir().join("xtask").join(name);
    fs::create_dir_all(target_dir().join("xtask")).context("creating target/xtask")?;
    let output = format!("type=docker,name={IMAGE},dest={}", archive.display());
    let label = format!("org.opencontainers.image.revision={commit}");
    process::run(&mut docker_build(
        arch,
        DockerStage::Runtime,
        &output,
        &["--label", &label],
    ))?;
    copy(&archive, out)
}

/// A stage of `packaging/docker/Dockerfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockerStage {
    /// The binary and the `.deb`, under their `dist/` names.
    Artifacts,
    /// The image.
    Runtime,
}

impl DockerStage {
    fn name(self) -> &'static str {
        match self {
            Self::Artifacts => "artifacts",
            Self::Runtime => "runtime",
        }
    }
}

/// `docker buildx build` of `stage` for `arch` to `output`, with `extra`
/// options.
fn docker_build(arch: Arch, stage: DockerStage, output: &str, extra: &[&str]) -> Command {
    let platform = format!("linux/{}", arch.name());
    let mut command = process::command(
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
            output,
        ],
    );
    command.args(extra).arg(".");
    command
}

/// The bundle, then `packaging/webos/build-ipk.sh`, which prints the path of
/// the package it built.
fn tv(flavour: Flavour, out: &Path) -> Result<()> {
    ensure!(
        std::env::var_os("WEBOS_SDK").is_some(),
        "set WEBOS_SDK to the relocated webOS SDK (docs/developers/packaging.md#webos)"
    );
    npm::run(Script::Bundle)?;
    let dir = target_dir()
        .join("xtask")
        .join(format!("tv-{}", flavour.name()));
    empty_dir(&dir)?;
    let built = process::output(&mut build_ipk(flavour, &dir))?;
    copy(Path::new(&built), out)
}

fn build_ipk(flavour: Flavour, dir: &Path) -> Command {
    let mut command = process::command(
        "bash",
        &["packaging/webos/build-ipk.sh", flavour.name(), "--out"],
    );
    command.arg(dir);
    command
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
            .map(|target| target.kind().file_name(Arch::Arm64))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "prysm-headless-macos-arm64",
                "prysm-headless-linux-arm64",
                "prysm-headless-linux-amd64",
                "prysm-headless-linux-arm64.deb",
                "prysm-headless-linux-amd64.deb",
                "prysm-headless-linux-arm64.tar",
                "prysm-headless-linux-amd64.tar",
                "prysm-tv-full.ipk",
                "prysm-tv-slim.ipk",
            ]
        );
        assert_eq!(
            Kind::Mac.file_name(Arch::Amd64),
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
                "deb-amd64",
                "image-arm64",
                "image-amd64",
                "tv-full",
                "tv-slim",
            ]
        );
    }

    #[test]
    fn docker_builds_one_stage_for_one_platform() {
        let command = docker_build(
            Arch::Amd64,
            DockerStage::Artifacts,
            "type=local,dest=/tmp/out",
            &[],
        );
        assert_eq!(
            process::display(&command),
            "docker buildx build --file packaging/docker/Dockerfile --platform linux/amd64 \
             --target artifacts --output type=local,dest=/tmp/out ."
        );
        assert_eq!(command.get_current_dir(), Some(root()));
    }

    #[test]
    fn options_come_before_the_context() {
        let command = docker_build(
            Arch::Arm64,
            DockerStage::Runtime,
            "type=docker",
            &["--label", "a=b"],
        );
        assert!(
            process::display(&command)
                .ends_with("--target runtime --output type=docker --label a=b ."),
            "{}",
            process::display(&command)
        );
    }

    #[test]
    fn tv_packages_are_built_into_a_directory_of_their_own() {
        let command = build_ipk(Flavour::Slim, Path::new("/repo/target/xtask/tv-slim"));
        assert_eq!(
            process::display(&command),
            "bash packaging/webos/build-ipk.sh slim --out /repo/target/xtask/tv-slim"
        );
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
