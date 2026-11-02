use anyhow::{Result, ensure};
use prysm_webos_protocol::{Header, PixelFormat};

pub fn header(width: u32, height: u32, format: PixelFormat) -> Result<Header> {
    let mut header = Header {
        width,
        height,
        format,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 0,
    };
    header.payload_len = header.raw_len()?;
    Ok(header)
}

/// Copy active rows without padding, zero-filling, or per-frame allocation.
pub fn copy_plane(
    source: &[u8],
    stride: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    ensure!(
        rows > 0 && rows <= 1080 && row_bytes > 0 && stride >= row_bytes && stride <= 32768,
        "invalid native plane geometry"
    );
    ensure!(
        source.len() >= stride * (rows - 1) + row_bytes,
        "short native plane"
    );
    output.reserve(row_bytes * rows);
    for row in source.chunks(stride).take(rows) {
        output.extend_from_slice(&row[..row_bytes]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_padded_rows_and_rejects_invalid_planes() {
        let input = [1, 2, 3, 99, 4, 5, 6, 99];
        let mut output = Vec::new();
        copy_plane(&input, 4, 3, 2, &mut output).unwrap();
        assert_eq!(output, [1, 2, 3, 4, 5, 6]);
        assert!(copy_plane(&input[..2], 4, 3, 2, &mut output).is_err());
        assert!(copy_plane(&input, 0, 3, 2, &mut output).is_err());
    }
}
