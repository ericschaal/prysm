//! Splitting an Annex B access unit into its NAL units.
use crate::{H264Error, MAX_ACCESS_UNIT_BYTES, ensure};

/// The NAL units of `access_unit`, each without its start code; an item is
/// an error for an empty NAL unit.
///
/// # Errors
/// [`H264Error::Bitstream`] when `access_unit` is longer than
/// [`MAX_ACCESS_UNIT_BYTES`] or does not begin with a start code.
pub(crate) fn nal_units(
    access_unit: &[u8],
) -> Result<impl Iterator<Item = Result<&[u8], H264Error>>, H264Error> {
    ensure!(
        access_unit.len() <= MAX_ACCESS_UNIT_BYTES,
        "access unit of {} bytes",
        access_unit.len()
    );
    ensure!(
        access_unit.starts_with(&[0, 0, 1]) || access_unit.starts_with(&[0, 0, 0, 1]),
        "not an Annex B access unit"
    );
    // Each unit begins with the three-byte start code `00 00 01`.
    Ok(openh264::nal_units(access_unit).map(|unit| {
        unit.get(3..)
            .filter(|nal| !nal.is_empty())
            .ok_or_else(|| H264Error::Bitstream("empty NAL".into()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(access_unit: &[u8]) -> Result<Vec<&[u8]>, H264Error> {
        nal_units(access_unit)?.collect()
    }

    #[test]
    fn units_lose_their_start_codes() {
        assert_eq!(
            units(b"\x00\x00\x00\x01\x67\x42\x00\x00\x01\x68\x00\x00\x01\x65\x88").unwrap(),
            [&b"\x67\x42"[..], b"\x68", b"\x65\x88"]
        );
        assert_eq!(units(b"\x00\x00\x01\x09\xf0").unwrap(), [b"\x09\xf0"]);
    }

    #[test]
    fn malformed_access_units_are_refused() {
        let oversized = [&[0, 0, 1][..], &vec![1; MAX_ACCESS_UNIT_BYTES]].concat();
        for access_unit in [
            &b""[..],
            b"\x00\x01\x09",
            b"\x09\x00\x00\x01\x09",
            b"\x00\x00\x01",
            b"\x00\x00\x01\x09\x00\x00\x01",
            &oversized,
        ] {
            assert!(
                matches!(units(access_unit), Err(H264Error::Bitstream(_))),
                "{access_unit:02x?}"
            );
        }
    }
}
