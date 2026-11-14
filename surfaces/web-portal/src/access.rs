//! Construction-time validation and the per-request origin policy with CORS.
use crate::FrontendAsset;
use crate::response::{self, Response, failure};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::http::uri::Authority;
use hyper::{StatusCode, Uri};
use prysm_ports::ValidationError;
use prysm_protocol::FailureKind;

/// Which browser pages, besides the portal's own, may call the API. Requests
/// without an `Origin` header (curl, native clients) are always served; there
/// is no authentication.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Access {
    /// Only pages the portal served itself.
    #[default]
    SameOrigin,
    /// Also these exact origins, such as `http://192.168.1.20:3000`, with
    /// CORS. The entry `null` allows pages without an origin of their own:
    /// packaged webOS apps (`file://`), but also sandboxed frames on any site.
    Origins(Vec<String>),
    /// Every origin, including `null` (`file://` pages, sandboxed frames), with
    /// CORS: any page the user's browser opens can then drive the lights.
    AnyOrigin,
}

impl Access {
    /// Origins in browser serialization: lowercase, no default port.
    pub(crate) fn validated(self) -> Result<Self, ValidationError> {
        match self {
            Self::Origins(origins) => origins
                .iter()
                .map(|origin| normalize(origin))
                .collect::<Result<_, _>>()
                .map(Self::Origins),
            other => Ok(other),
        }
    }
}

/// The opaque origin browsers send for `file://` pages and sandboxed frames.
const NULL: &str = "null";

fn normalize(origin: &str) -> Result<String, ValidationError> {
    let invalid = || {
        ValidationError::new(
            "portal.access",
            format!(
                "`{origin}` is not an origin such as http://192.168.1.20:3000 \
                 or `null` (use AnyOrigin for `*`)"
            ),
        )
    };
    if origin == NULL {
        return Ok(NULL.into());
    }
    let lower = origin.to_ascii_lowercase();
    let uri: Uri = lower.parse().map_err(|_| invalid())?;
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return Err(invalid());
    };
    if authority.host().is_empty()
        || authority.as_str().contains('@')
        || lower.ends_with('/')
        || uri
            .path_and_query()
            .is_some_and(|path| path.as_str() != "/")
    {
        return Err(invalid());
    }
    let default_port = match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    Ok(
        if authority.port_u16().is_some() && authority.port_u16() == default_port {
            format!("{scheme}://{}", authority.host())
        } else {
            lower
        },
    )
}

pub(crate) fn validate_assets(assets: &[FrontendAsset]) -> Result<(), ValidationError> {
    for (index, asset) in assets.iter().enumerate() {
        let path = asset.path;
        if !path.starts_with('/')
            || path.starts_with("/api/")
            || path.contains(['?', '#', '%', '\\'])
            || path.split('/').any(|part| part == "." || part == "..")
            || assets[..index].iter().any(|other| other.path == path)
            || HeaderValue::from_str(asset.content_type).is_err()
        {
            return Err(ValidationError::new(
                "portal.assets",
                format!("invalid or duplicate asset `{path}`"),
            ));
        }
    }
    Ok(())
}

/// The origin policy for an API request. `Ok(None)`: no `Origin` or the
/// portal's own; `Ok(Some(origin))`: an allowed cross origin that gets CORS
/// headers; `Err`: `403 forbidden`. Forwarded headers are ignored.
pub(crate) fn check(
    access: &Access,
    headers: &HeaderMap,
) -> Result<Option<HeaderValue>, Box<Response>> {
    let denied = || {
        Box::new(failure(
            StatusCode::FORBIDDEN,
            FailureKind::Forbidden,
            "origin denied",
        ))
    };
    let mut values = headers.get_all(header::ORIGIN).iter();
    let origin = match (values.next(), values.next()) {
        (None, _) => return Ok(None),
        (Some(origin), None) => origin,
        (Some(_), Some(_)) => return Err(denied()),
    };
    let text = origin.to_str().map_err(|_| denied())?;
    if same_origin(text, headers) {
        return Ok(None);
    }
    let allowed = match access {
        Access::SameOrigin => false,
        Access::Origins(origins) => origins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(text)),
        Access::AnyOrigin => true,
    };
    if allowed {
        Ok(Some(origin.clone()))
    } else {
        Err(denied())
    }
}

/// `Origin` is `http://` plus the `Host` header (the portal serves plain HTTP).
fn same_origin(origin: &str, headers: &HeaderMap) -> bool {
    let mut hosts = headers.get_all(header::HOST).iter();
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return false;
    };
    let (Some(origin), Ok(host)) = (origin.strip_prefix("http://"), host.to_str()) else {
        return false;
    };
    origin.eq_ignore_ascii_case(host)
        || host.parse::<Authority>().is_ok_and(|authority| {
            authority.port_u16() == Some(80) && origin.eq_ignore_ascii_case(authority.host())
        })
}

/// The answer to a CORS preflight from an allowed cross origin.
pub(crate) fn preflight(origin: HeaderValue, request: &HeaderMap) -> Response {
    let mut response = response::empty(StatusCode::NO_CONTENT);
    allow(&mut response, origin);
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, PUT, DELETE"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    // Chromium's Private Network Access asks before a page reaches a LAN address.
    if request
        .get("access-control-request-private-network")
        .is_some_and(|value| value == "true")
    {
        headers.insert(
            "access-control-allow-private-network",
            HeaderValue::from_static("true"),
        );
    }
    response
}

/// Let `origin` read `response`.
pub(crate) fn allow(response: &mut Response, origin: HeaderValue) {
    response
        .headers_mut()
        .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
}
