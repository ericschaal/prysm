use prysm_webos_protocol::{HEADER_LEN, Header, h264::Decoder};
use std::{error::Error, fs::File, io::Read, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::args_os().nth(1).ok_or("wire directory required")?);
    for (name, count, width, height) in [
        ("hardware-640x280", 300, 640, 280),
        ("hardware-320x180", 35, 320, 180),
        ("software-640x280", 35, 640, 280),
        ("software-320x180", 35, 320, 180),
    ] {
        let mut file = File::open(root.join(format!("{name}.wire")))?;
        let mut decoder = Decoder::new()?;
        for i in 0..count {
            let mut bytes = [0; HEADER_LEN];
            file.read_exact(&mut bytes)?;
            let header = Header::decode(&bytes)?;
            assert_eq!((header.width, header.height), (width, height));
            let mut payload = vec![0; header.payload_len];
            file.read_exact(&mut payload)?;
            let rgb = decoder.decode(header, &payload)?;
            assert_eq!(rgb.len(), (width * height * 3) as usize);
            if i == 0 {
                Decoder::new()?.decode(header, &payload)?;
            }
        }
        assert_eq!(file.read(&mut [0])?, 0, "extra wire bytes in {name}");
        println!("{name}: all {count} wire frames decoded with unchanged receiver; first frame independently decodable");
    }
    Ok(())
}
