//! Names and paths of each package. Fixed at build time by the binary, so a
//! supervisor can never be paired with the other package's worker.
use crate::install::Layout;
use crate::machine::Spec;
use crate::service::Config;
use crate::state::Flavor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Where the full worker serves the portal.
pub const PORTAL: &str = "0.0.0.0:8787";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub flavor: Flavor,
    /// The TV app, the only caller allowed to change anything.
    pub app_id: String,
    /// The Luna name this supervisor registers.
    pub service: String,
    /// The worker binary.
    pub worker: PathBuf,
    /// Private (root, 0700) state: `tv.json` and, for full, `settings.json`.
    pub state_dir: PathBuf,
    pub layout: Layout,
}

fn names(flavor: Flavor) -> (&'static str, &'static str, &'static str) {
    match flavor {
        Flavor::Full => ("io.prysm.tv", "prysm-webos", "prysm-webos-service"),
        Flavor::Slim => (
            "io.prysm.tv.slim",
            "prysm-webos-slim",
            "prysm-webos-slim-service",
        ),
    }
}

impl Identity {
    /// The installed package on a TV.
    #[must_use]
    pub fn production(flavor: Flavor) -> Self {
        let (app, worker, supervisor) = names(flavor);
        let service = format!("{app}.service");
        let dir = PathBuf::from("/media/developer/apps/usr/palm/services").join(&service);
        Self {
            flavor,
            app_id: app.into(),
            worker: dir.join(worker),
            state_dir: PathBuf::from("/var/lib").join(app),
            layout: Layout {
                id: app.into(),
                manifests: "/var/luna-service2/manifests.d".into(),
                supervisor: dir.join(supervisor),
                service_dir: dir,
                hook: PathBuf::from("/var/lib/webosbrew/init.d").join(app),
                scanner: "/usr/sbin/ls-control".into(),
                luna_send: "/usr/bin/luna-send".into(),
                hook_log: format!("/tmp/{app}-setup.log").into(),
            },
            service,
        }
    }

    /// An isolated probe: every path under `root`, a separate app and
    /// service name, binaries taken from next to `binaries`.
    #[must_use]
    pub fn sandbox(flavor: Flavor, root: &Path, binaries: &Path, service: Option<String>) -> Self {
        let (app, worker, supervisor) = names(flavor);
        let app = format!("{app}.probe");
        let service = service.unwrap_or_else(|| format!("{app}.service"));
        Self {
            flavor,
            worker: binaries.join(worker),
            state_dir: root.join("state"),
            layout: Layout {
                id: app.clone(),
                manifests: root.join("manifests.d"),
                service_dir: binaries.to_path_buf(),
                supervisor: binaries.join(supervisor),
                hook: root.join("init.d").join(&app),
                scanner: root.join("ls-control"),
                luna_send: root.join("luna-send"),
                hook_log: root.join("setup.log"),
            },
            app_id: app,
            service,
        }
    }

    #[must_use]
    pub fn preferences(&self) -> PathBuf {
        self.state_dir.join("tv.json")
    }

    /// # Panics
    /// Never: [`PORTAL`] is a valid address.
    #[must_use]
    pub fn config(&self) -> Config {
        Config {
            flavor: self.flavor,
            app_id: self.app_id.clone(),
            program: self.worker.clone(),
            spec: match self.flavor {
                Flavor::Full => Spec::Full {
                    settings: self.state_dir.join("settings.json"),
                    listen: PORTAL.parse::<SocketAddr>().expect("valid portal address"),
                },
                Flavor::Slim => Spec::Slim,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_never_share_names_or_paths() {
        let full = Identity::production(Flavor::Full);
        let slim = Identity::production(Flavor::Slim);
        assert_eq!(full.service, "io.prysm.tv.service");
        assert_eq!(slim.service, "io.prysm.tv.slim.service");
        assert!(full.worker.ends_with("io.prysm.tv.service/prysm-webos"));
        assert!(
            slim.worker
                .ends_with("io.prysm.tv.slim.service/prysm-webos-slim")
        );
        assert_ne!(full.state_dir, slim.state_dir);
        assert_ne!(full.layout.hook, slim.layout.hook);
        assert!(matches!(full.config().spec, Spec::Full { .. }));
        assert_eq!(slim.config().spec, Spec::Slim);
    }

    #[test]
    fn sandbox_stays_under_its_root() {
        let id = Identity::sandbox(Flavor::Slim, Path::new("/tmp/p"), Path::new("/opt/b"), None);
        assert_eq!(id.app_id, "io.prysm.tv.slim.probe");
        assert_eq!(id.service, "io.prysm.tv.slim.probe.service");
        assert!(id.preferences().starts_with("/tmp/p"));
        assert!(id.layout.manifests.starts_with("/tmp/p"));
        assert!(id.layout.hook.starts_with("/tmp/p"));
        assert_eq!(id.worker, Path::new("/opt/b/prysm-webos-slim"));
    }
}
