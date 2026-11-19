//! Finds the portal bundles `npm run bundle` writes to `frontend/dist`. A
//! bundle that is there sets `bundle_<name>`, and `src/assets.rs` embeds it.
//! A missing one is the "not built" page in a debug build, and an error in
//! any other: a binary built for shipping always carries its portal.
use std::env;
use std::path::Path;

/// The bundles the hosts serve, by their directory in `frontend/dist`.
const BUNDLES: [&str; 2] = ["headless", "webos"];

fn main() {
    let root = Path::new(&env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo")).join("../..");
    // "debug" for every profile that inherits from dev, "release" for the
    // ones that inherit from release (release, bench and webos). Shipping
    // profiles inherit from release, whatever else they change.
    let debug = env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    for name in BUNDLES {
        let cfg = format!("bundle_{name}");
        println!("cargo::rustc-check-cfg=cfg({cfg})");
        let relative = format!("frontend/dist/{name}/web-portal/assets.rs");
        let list = root.join(&relative);
        // A new bundle rewrites the list; the files it names are tracked by
        // rustc once embedded.
        println!("cargo::rerun-if-changed={}", list.display());
        if list.is_file() {
            println!("cargo::rustc-cfg={cfg}");
        } else if debug {
            // Cargo reruns a script while a path it watches is missing. This
            // one never exists, so a stub build looks again on every build
            // (well under a second), even for a bundle copied back with old
            // timestamps, and the warning stays visible.
            let never = Path::new(&env::var_os("OUT_DIR").expect("set by Cargo")).join("never");
            println!("cargo::rerun-if-changed={}", never.display());
            println!(
                "cargo::warning={relative} is missing: the portal shows a \"not built\" page. \
                 Run `npm --prefix frontend ci && npm --prefix frontend run bundle`, then build again."
            );
        } else {
            println!(
                "cargo::error={relative} is missing, and only a debug build may go without the \
                 portal. Run `npm --prefix frontend ci && npm --prefix frontend run bundle` first."
            );
        }
    }
}
