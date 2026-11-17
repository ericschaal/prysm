//! The Docker targets: stages of `packaging/docker/Dockerfile` built with
//! `docker buildx build`. Every build also gets the words of
//! `XTASK_BUILDX_ARGS`, when set; CI passes its layer cache options that way.
use super::{Arch, empty_dir};
use crate::process::{self, target_dir};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The name `docker load` gives the image, which
/// `packaging/release/publish.sh` and `packaging/docker/smoke.sh` use.
const IMAGE: &str = "prysm-headless:local";

/// A stage of `packaging/docker/Dockerfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The binary and the `.deb`, under their `dist/` names.
    Artifacts,
    /// The image.
    Runtime,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Artifacts => "artifacts",
            Self::Runtime => "runtime",
        }
    }
}

/// The Docker builds of one `dist` run.
pub struct Docker {
    /// More `docker buildx build` options: the words of `XTASK_BUILDX_ARGS`.
    buildx_args: Vec<String>,
    /// Architectures whose `artifacts` stage this run has exported.
    exported: Vec<Arch>,
}

impl Docker {
    pub fn from_env() -> Self {
        Self {
            buildx_args: std::env::var("XTASK_BUILDX_ARGS")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            exported: Vec::new(),
        }
    }

    /// Exports the `artifacts` stage for `arch` (the binary and the `.deb`)
    /// into `target/xtask/docker-<arch>`, once per run, and returns that
    /// directory.
    pub fn artifacts(&mut self, arch: Arch) -> Result<PathBuf> {
        let dir = target_dir()
            .join("xtask")
            .join(format!("docker-{}", arch.name()));
        if !self.exported.contains(&arch) {
            empty_dir(&dir)?;
            let output = format!("type=local,dest={}", dir.display());
            process::run(build(arch, Stage::Artifacts, &output, &self.buildx_args))?;
            self.exported.push(arch);
        }
        Ok(dir)
    }

    /// The `runtime` stage for `arch` as an image archive at `out`, labelled
    /// with `commit`.
    pub fn image(&self, arch: Arch, commit: &str, out: &Path) -> Result<()> {
        let output = format!("type=docker,name={IMAGE},dest={}", out.display());
        let mut options = vec![
            "--label".to_owned(),
            format!("org.opencontainers.image.revision={commit}"),
        ];
        options.extend(self.buildx_args.iter().cloned());
        process::run(build(arch, Stage::Runtime, &output, &options))
    }
}

/// `docker buildx build` of `stage` for `arch` to `output`, with `options`.
fn build(arch: Arch, stage: Stage, output: &str, options: &[String]) -> Command {
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
    command.args(options).arg(".");
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::root;

    #[test]
    fn docker_builds_one_stage_for_one_platform() {
        let command = build(
            Arch::Amd64,
            Stage::Artifacts,
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
        let options = ["--label", "a=b", "--cache-from", "type=gha"].map(String::from);
        let command = build(Arch::Arm64, Stage::Runtime, "type=docker", &options);
        assert!(
            process::display(&command)
                .ends_with("--output type=docker --label a=b --cache-from type=gha ."),
            "{}",
            process::display(&command)
        );
    }
}
