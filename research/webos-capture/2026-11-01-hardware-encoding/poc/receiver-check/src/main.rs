use prysm_webos_protocol::{Codec, Header, PixelFormat, h264::Decoder};
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("access-unit directory required")?,
    );
    let original = fs::read(directory.join("original-00.au"))?;
    let header = Header {
        width: 640,
        height: 360,
        format: PixelFormat::Rgb,
        codec: Codec::H264,
        payload_len: original.len(),
    };
    let error = Decoder::new()?
        .decode(header, &original)
        .expect_err("original SPS must fail dimensions");
    assert!(
        error.to_string().contains("SPS dimensions disagree"),
        "{error}"
    );
    println!("Original at 640x360 rejected: {error}");
    let error = Decoder::new()?
        .decode(
            Header {
                height: 368,
                ..header
            },
            &original,
        )
        .expect_err("original SPS must fail colorimetry even with matching dimensions");
    assert!(
        error.to_string().contains("limited-range BT.709"),
        "{error}"
    );
    println!("Original at 640x368 rejected: {error}");

    let mut decoder = Decoder::new()?;
    for index in 0..15 {
        let payload = fs::read(directory.join(format!("adjusted-{index:02}.au")))?;
        let pixels = decoder.decode(
            Header {
                payload_len: payload.len(),
                ..header
            },
            &payload,
        )?;
        assert_eq!(pixels.len(), 640 * 360 * 3);
        let expected = ((32 + index * 8 - 16) * 255 / 219) as i16;
        assert!(
            pixels
                .iter()
                .all(|&pixel| (i16::from(pixel) - expected).abs() <= 3),
            "decoded grayscale mismatch in frame {index}"
        );
    }
    println!(
        "All 15 SPS-adjusted hardware access units passed unchanged Prysm validation and RGB content assertions."
    );
    Ok(())
}
