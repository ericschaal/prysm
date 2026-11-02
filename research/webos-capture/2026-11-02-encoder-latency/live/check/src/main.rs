use prysm_webos_protocol::{Codec, Header, PixelFormat, h264::{Decoder, hardware::normalize_access_unit}};
use std::{error::Error,fs,path::PathBuf};
fn main()->Result<(),Box<dyn Error>> {
    let root=PathBuf::from(std::env::args_os().nth(1).ok_or("evidence directory required")?);
    for case in ["0-parsed","1-direct","2-parsed"] {
        let mut decoder=Decoder::new()?;
        let mut normalized=Vec::new();
        for i in 0..45 {
            let raw=fs::read(root.join(case).join(format!("frame-{i:03}.au")))?;
            normalize_access_unit(&raw,640,360,i==0,&mut normalized)?;
            let pixels=decoder.decode(Header{width:640,height:360,format:PixelFormat::Rgb,codec:Codec::H264,payload_len:normalized.len()},&normalized)?;
            let y=1.16438356*f64::from(32+(i%16)*8-16);
            let u=if i%2==0 {-32.0} else {32.0}; let v=-u;
            let expected=[y+1.79274107*v,y-0.21324861*u-0.53290933*v,y+2.11240179*u].map(|x|x.round().clamp(0.0,255.0) as i16);
            assert_eq!(pixels.len(),640*360*3);
            for pixel in pixels.chunks_exact(3) {
                for c in 0..3 {assert!((i16::from(pixel[c])-expected[c]).abs()<=4,"{case} frame{i}: {pixel:?} vs {expected:?}");}
            }
        }
        println!("{case}: all45 complete access units normalized and decoded; every RGB pixel matches its submitted frame within4");
    }
    Ok(())
}
