//! The product version: the repository's `VERSION` file, which Release Please
//! owns (see `docs/developers/releasing.md`). It is the only place the version
//! is written; the crates themselves are all `0.0.0` and never published.
//!
//! `include_str!` makes Cargo rebuild this crate when `VERSION` changes. A
//! file that is not `MAJOR.MINOR.PATCH`, optionally followed by `-pre-release`
//! or `+build`, fails the build.

/// The full version, such as `1.2.3` or `1.0.0-alpha.0`.
pub const VERSION: &str = include_str!("../../../VERSION").trim_ascii_end();

/// `MAJOR.MINOR.PATCH` without a pre-release or build suffix: what the TV
/// packages' `appinfo.json` and Luna manifests accept.
pub const RELEASE: &str = release(VERSION);

const fn release(version: &str) -> &str {
    let bytes = version.as_bytes();
    let mut end = 0;
    let mut dots = 0;
    let mut digits = 0;
    while end < bytes.len() && bytes[end] != b'-' && bytes[end] != b'+' {
        match bytes[end] {
            b'.' if digits > 0 => {
                dots += 1;
                digits = 0;
            }
            b'0'..=b'9' => digits += 1,
            _ => panic!("VERSION must be MAJOR.MINOR.PATCH[-pre-release][+build]"),
        }
        end += 1;
    }
    assert!(
        dots == 2 && digits > 0,
        "VERSION must be MAJOR.MINOR.PATCH[-pre-release][+build]"
    );
    assert!(
        end == bytes.len() || end + 1 < bytes.len(),
        "VERSION has an empty suffix"
    );
    match core::str::from_utf8(bytes.split_at(end).0) {
        Ok(release) => release,
        Err(_) => panic!("VERSION is not UTF-8"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_drops_the_suffix() {
        assert_eq!(release("1.2.3"), "1.2.3");
        assert_eq!(release("1.0.0-alpha.0"), "1.0.0");
        assert_eq!(release("10.20.30+build.7"), "10.20.30");
    }

    #[test]
    fn the_version_file_is_one_trimmed_line() {
        assert!(!VERSION.is_empty());
        assert!(!VERSION.contains(char::is_whitespace), "{VERSION:?}");
        assert!(VERSION.starts_with(RELEASE));
    }

    #[test]
    #[should_panic(expected = "MAJOR.MINOR.PATCH")]
    fn two_parts_are_not_a_version() {
        let _ = release("1.2");
    }

    #[test]
    #[should_panic(expected = "MAJOR.MINOR.PATCH")]
    fn a_leading_v_is_not_a_version() {
        let _ = release("v1.2.3");
    }
}
