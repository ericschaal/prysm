//! `cargo xtask dist`: artifacts under fixed names in `dist/`. The Docker
//! targets are built in [`docker`].
mod docker;

use crate::manifest::{Entry, Manifest, sha256};
use crate::npm::{self, Script};
use crate::process::{self, root, target_dir};
use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use docker::Docker;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime};

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

    /// Fails when this machine cannot build `self`.
    fn check_prerequisites(self) -> Result<()> {
        match self {
            Self::Mac => ensure!(
                cfg!(target_os = "macos"),
                "the mac target builds only on macOS"
            ),
            Self::Tv(_) => ensure!(
                std::env::var_os("WEBOS_SDK").is_some(),
                "set WEBOS_SDK to the relocated webOS SDK (docs/developers/packaging.md#webos)"
            ),
            Self::Binary(_) | Self::Deb(_) | Self::Image(_) => {}
        }
        Ok(())
    }
}

/// Builds each target in turn and records it in `dist/manifest.json` as soon
/// as it is done, so an earlier success is kept when a later target fails.
pub fn run(targets: &[Target]) -> Result<()> {
    // Before any build, so a missing prerequisite fails in seconds.
    for target in targets {
        target.kind().check_prerequisites()?;
    }
    let dist = root().join("dist");
    fs::create_dir_all(&dist).with_context(|| format!("creating {}", dist.display()))?;
    // Read first, so a broken manifest fails before a long build.
    let manifest_path = dist.join("manifest.json");
    let mut manifest = Manifest::read(&manifest_path)?;
    manifest.retain(|file| dist.join(file).is_file());
    let mut builder = Builder {
        host: Arch::host()?,
        source: Source::current()?,
        docker: Docker::from_env(),
        bundled: false,
    };
    let started = Instant::now();
    let mut done = Vec::new();
    for &target in targets {
        if done.contains(&target) {
            continue;
        }
        let target_started = Instant::now();
        let kind = target.kind();
        let name = kind.file_name(builder.host);
        let out = dist.join(&name);
        builder
            .build(kind, &out)
            .with_context(|| format!("building {name}"))?;
        let entry = builder.source.entry(sha256(&out)?, SystemTime::now());
        manifest.insert(name.clone(), entry);
        manifest.write(&manifest_path)?;
        let took = process::duration(target_started.elapsed());
        eprintln!("Wrote dist/{name} ({took})");
        done.push(target);
    }
    let took = process::duration(started.elapsed());
    let plural = if done.len() == 1 { "" } else { "s" };
    eprintln!("Built {} artifact{plural} ({took}).", done.len());
    if builder.source.dirty {
        eprintln!(
            "warning: the working tree has uncommitted changes; \
             dist/manifest.json records these artifacts as dirty"
        );
    }
    Ok(())
}

/// State shared by the targets of one `dist` run.
struct Builder {
    host: Arch,
    source: Source,
    docker: Docker,
    /// Whether this run has bundled the frontend.
    bundled: bool,
}

impl Builder {
    /// Builds the artifact of `kind` at `out`.
    fn build(&mut self, kind: Kind, out: &Path) -> Result<()> {
        match kind {
            Kind::Mac => {
                self.bundle()?;
                mac(out)
            }
            Kind::Binary(arch) | Kind::Deb(arch) => {
                let exported = self.docker.artifacts(arch)?;
                copy(&exported.join(kind.file_name(self.host)), out)
            }
            Kind::Image(arch) => {
                let partial = temporary(out);
                self.docker.image(arch, &self.source.commit, &partial)?;
                rename(&partial, out)
            }
            Kind::Tv(flavour) => {
                self.bundle()?;
                tv(flavour, out)
            }
        }
    }

    /// `npm run bundle`, once per run. The Mac binary and the TV packages
    /// take it from `frontend/dist`; the Dockerfile bundles on its own.
    fn bundle(&mut self) -> Result<()> {
        if !self.bundled {
            npm::run(Script::Bundle)?;
            self.bundled = true;
        }
        Ok(())
    }
}

/// A release build of prysm-headless for this Mac.
fn mac(out: &Path) -> Result<()> {
    process::run(process::command(
        "cargo",
        &["build", "--release", "--locked", "-p", "prysm-headless"],
    ))?;
    copy(&target_dir().join("release/prysm-headless"), out)
}

/// `packaging/webos/build-ipk.sh`, which prints the path of the package it
/// built.
fn tv(flavour: Flavour, out: &Path) -> Result<()> {
    let dir = target_dir()
        .join("xtask")
        .join(format!("tv-{}", flavour.name()));
    empty_dir(&dir)?;
    let built = process::output(build_ipk(flavour, &dir))?;
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
        let git = |args: &[&str]| process::query(process::command("git", args));
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

/// `path` with `.tmp` appended. Artifacts are written there and then
/// renamed, so `dist/` never holds half a file under an artifact's name.
fn temporary(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    temporary.into()
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).with_context(|| format!("renaming {} to {}", from.display(), to.display()))
}

/// Copies `from` to `to` through a temporary name, keeping the permissions;
/// macOS may also kill a signed binary rewritten in place.
fn copy(from: &Path, to: &Path) -> Result<()> {
    let temporary = temporary(to);
    fs::copy(from, &temporary)
        .with_context(|| format!("copying {} to {}", from.display(), temporary.display()))?;
    rename(&temporary, to)
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
