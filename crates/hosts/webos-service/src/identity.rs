//! Names and paths of the installed app. Both packages install under one
//! identity, so installing one replaces the other; the flavor (and with it
//! the worker the supervisor runs) is fixed at build time by the binary.
use crate::install::Layout;
use crate::machine::Spec;
use crate::service::Config;
use crate::state::Flavor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

/// Where the full worker serves the portal.
pub const PORTAL: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8787);
/// The TV app of both packages.
pub const APP_ID: &str = "io.prysm.tv";
/// The supervisor's installed file name in both packages.
pub const SUPERVISOR: &str = "prysm-webos-service";
/// The worker's installed file name in both packages.
pub const WORKER: &str = "prysm-webos-worker";

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

/// The cargo binaries `(supervisor, worker)` of each flavor, which packaging
/// installs as [`SUPERVISOR`] and [`WORKER`]; a sandbox runs them in place.
#[must_use]
pub fn build_names(flavor: Flavor) -> (&'static str, &'static str) {
    match flavor {
        Flavor::Full => ("prysm-webos-service", "prysm-webos"),
        Flavor::Slim => ("prysm-webos-slim-service", "prysm-webos-slim"),
    }
}

impl Identity {
    /// The installed app on a TV: identical for both flavors except `flavor`.
    #[must_use]
    pub fn production(flavor: Flavor) -> Self {
        let service = format!("{APP_ID}.service");
        let dir = PathBuf::from("/media/developer/apps/usr/palm/services").join(&service);
        Self {
            flavor,
            app_id: APP_ID.into(),
            worker: dir.join(WORKER),
            state_dir: PathBuf::from("/var/lib").join(APP_ID),
            layout: Layout {
                id: APP_ID.into(),
                manifests: "/var/luna-service2/manifests.d".into(),
                supervisor: dir.join(SUPERVISOR),
                service_dir: dir,
                hook: PathBuf::from("/var/lib/webosbrew/init.d").join(APP_ID),
                scanner: "/usr/sbin/ls-control".into(),
                luna_send: "/usr/bin/luna-send".into(),
                hook_log: format!("/tmp/{APP_ID}-setup.log").into(),
            },
            service,
        }
    }

    /// An isolated probe: every path under `root`, a separate app and
    /// service name, the flavor's cargo binaries taken from `binaries`.
    #[must_use]
    pub fn sandbox(flavor: Flavor, root: &Path, binaries: &Path, service: Option<String>) -> Self {
        let (supervisor, worker) = build_names(flavor);
        let app = format!("{APP_ID}.probe");
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

    #[must_use]
    pub fn config(&self) -> Config {
        Config {
            flavor: self.flavor,
            app_id: self.app_id.clone(),
            program: self.worker.clone(),
            spec: match self.flavor {
                Flavor::Full => Spec::Full {
                    settings: self.state_dir.join("settings.json"),
                    listen: PORTAL,
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
    fn both_flavors_install_under_one_identity() {
        let full = Identity::production(Flavor::Full);
        let slim = Identity::production(Flavor::Slim);
        assert_eq!(full.service, "io.prysm.tv.service");
        assert_eq!(full.app_id, "io.prysm.tv");
        assert!(
            full.worker
                .ends_with("io.prysm.tv.service/prysm-webos-worker")
        );
        assert!(
            full.layout
                .supervisor
                .ends_with("io.prysm.tv.service/prysm-webos-service")
        );
        assert_eq!(full.state_dir, Path::new("/var/lib/io.prysm.tv"));
        assert_eq!(
            full.layout.hook,
            Path::new("/var/lib/webosbrew/init.d/io.prysm.tv")
        );
        // Everything but the build-time flavor is shared.
        assert_eq!(
            Identity {
                flavor: Flavor::Full,
                ..slim.clone()
            },
            full
        );
        assert_eq!(full.preferences(), slim.preferences());
        assert!(matches!(full.config().spec, Spec::Full { .. }));
        assert_eq!(slim.config().spec, Spec::Slim);
        assert_eq!(slim.config().program, full.config().program);
    }

    #[test]
    fn sandbox_stays_under_its_root() {
        let id = Identity::sandbox(Flavor::Slim, Path::new("/tmp/p"), Path::new("/opt/b"), None);
        assert_eq!(id.app_id, "io.prysm.tv.probe");
        assert_eq!(id.service, "io.prysm.tv.probe.service");
        assert!(id.preferences().starts_with("/tmp/p"));
        assert!(id.layout.manifests.starts_with("/tmp/p"));
        assert!(id.layout.hook.starts_with("/tmp/p"));
        assert_eq!(id.worker, Path::new("/opt/b/prysm-webos-slim"));
        assert_eq!(
            id.layout.supervisor,
            Path::new("/opt/b/prysm-webos-slim-service")
        );
    }
}
