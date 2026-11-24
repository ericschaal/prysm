//! The portal bundles, embedded at compile time from the asset lists
//! `npm run bundle` writes to `frontend/dist/<host>/web-portal/assets.rs`.
//! Without one, a debug build serves [`NOT_BUILT`] in its place and a
//! release build fails (see `build.rs`).
use prysm_web_portal::{Caching, FrontendAsset};

/// The desktop hosts' portal (`prysm-headless`, `prysm-tui`): they register
/// the same adapters, so they share one bundle.
#[cfg(bundle_headless)]
pub const HEADLESS: &[FrontendAsset] =
    include!("../../../../frontend/dist/headless/web-portal/assets.rs");
#[cfg(not(bundle_headless))]
pub const HEADLESS: &[FrontendAsset] = NOT_BUILT;

/// The full TV worker's portal (`prysm-webos`).
#[cfg(bundle_webos)]
pub const WEBOS: &[FrontendAsset] =
    include!("../../../../frontend/dist/webos/web-portal/assets.rs");
#[cfg(not(bundle_webos))]
pub const WEBOS: &[FrontendAsset] = NOT_BUILT;

/// One page that says the frontend is not built and how to build it. The
/// API works without it.
pub const NOT_BUILT: &[FrontendAsset] = &[FrontendAsset {
    path: "/index.html",
    content_type: "text/html; charset=utf-8",
    bytes: include_bytes!("not-built.html"),
    gzip: None,
    caching: Caching::Never,
}];

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_web_portal::{Access, WebPortal};
    use std::net::TcpListener;

    fn serves(assets: &'static [FrontendAsset]) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        WebPortal::from_listener(listener, assets, Access::SameOrigin).unwrap();
        assert!(assets.iter().any(|asset| asset.path == "/index.html"));
    }

    #[test]
    fn the_not_built_page_is_a_valid_portal() {
        serves(NOT_BUILT);
    }

    /// The page has a head, where the portal puts the current state.
    fn has_head(assets: &[FrontendAsset]) -> bool {
        let page = assets.iter().find(|asset| asset.path == "/index.html");
        page.is_some_and(|page| page.bytes.windows(7).any(|window| window == b"</head>"))
    }

    #[test]
    #[cfg_attr(not(bundle_headless), ignore = "frontend/dist is not built")]
    fn the_desktop_portal_is_the_real_bundle() {
        serves(HEADLESS);
        assert!(HEADLESS.len() > NOT_BUILT.len());
        assert!(has_head(HEADLESS));
    }

    #[test]
    #[cfg_attr(not(bundle_webos), ignore = "frontend/dist is not built")]
    fn the_tv_portal_is_the_real_bundle() {
        serves(WEBOS);
        assert!(WEBOS.len() > NOT_BUILT.len());
        assert!(has_head(WEBOS));
    }
}
