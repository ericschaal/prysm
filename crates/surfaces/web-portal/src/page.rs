//! The page with the current state in it, so the portal draws that state
//! on its first render instead of waiting for the state stream.
use bytes::Bytes;
use serde::Serialize;
use std::fmt::Write;

/// The asset that is the page; `/` serves it too.
pub(crate) const PAGE: &str = "/index.html";
const HEAD_END: &[u8] = b"</head>";
/// The data block's start; its id is the one `embeddedState` reads in
/// `application-client/http.ts`.
const BLOCK_START: &str = r#"<script type="application/json" id="prysm-state">"#;
const BLOCK_END: &str = "</script>";

/// `page` with `state` in an inert JSON data block just before `</head>`.
/// A page without `</head>` (one that is not the portal) is sent as it is.
pub(crate) fn with_state(page: &'static [u8], state: &impl Serialize) -> Bytes {
    let at = page
        .windows(HEAD_END.len())
        .position(|window| window == HEAD_END);
    let (Some(at), Ok(json)) = (at, serde_json::to_string(state)) else {
        return Bytes::from_static(page);
    };
    let json = escape(&json);
    let mut body =
        Vec::with_capacity(page.len() + BLOCK_START.len() + json.len() + BLOCK_END.len());
    body.extend_from_slice(&page[..at]);
    body.extend_from_slice(BLOCK_START.as_bytes());
    body.extend_from_slice(json.as_bytes());
    body.extend_from_slice(BLOCK_END.as_bytes());
    body.extend_from_slice(&page[at..]);
    Bytes::from(body)
}

/// `json` that cannot end the element it is in: `<`, `>` and `&` (so no
/// `</script>` or `<!--`), and U+2028 and U+2029, become `\uXXXX`. JSON has
/// these only inside strings, where the escape means the same character.
fn escape(json: &str) -> String {
    let mut escaped = String::with_capacity(json.len());
    for character in json.chars() {
        if matches!(character, '<' | '>' | '&' | '\u{2028}' | '\u{2029}') {
            let _ = write!(escaped, "\\u{:04x}", u32::from(character));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const PORTAL: &[u8] =
        b"<!doctype html><html><head><title>Prysm</title></head><body></body></html>";

    /// The block's text, as the page reads it.
    fn block(page: &[u8]) -> &str {
        let page = std::str::from_utf8(page).unwrap();
        let start = page.find(BLOCK_START).unwrap() + BLOCK_START.len();
        let end = start + page[start..].find(BLOCK_END).unwrap();
        &page[start..end]
    }

    #[test]
    fn markup_in_a_string_cannot_end_the_block() {
        let state = json!({
            "name": "</script><script>alert(1)</script><!-- a & b \u{2028}\u{2029}",
            "path": "C:\\<dir>",
        });
        let page = with_state(PORTAL, &state);
        let text = block(&page);
        for raw in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(!text.contains(raw), "{raw:?} in {text}");
        }
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), state);
        // One block, and the page's own markup after it.
        let page = std::str::from_utf8(&page).unwrap();
        assert_eq!(page.matches("<script").count(), 1);
        assert!(page.ends_with("</script></head><body></body></html>"));
    }

    #[test]
    fn the_block_goes_before_the_end_of_the_head() {
        let page = with_state(PORTAL, &json!({ "revision": 3 }));
        assert_eq!(
            std::str::from_utf8(&page).unwrap(),
            r#"<!doctype html><html><head><title>Prysm</title><script type="application/json" id="prysm-state">{"revision":3}</script></head><body></body></html>"#
        );
    }

    #[test]
    fn a_page_without_a_head_is_sent_as_it_is() {
        let page: &[u8] = b"<!doctype html><title>Not built</title><h1>Build it</h1>";
        assert_eq!(with_state(page, &json!({})), page);
    }
}
