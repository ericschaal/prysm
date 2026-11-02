use prysm_webos_protocol::{Codec, Header, PixelFormat, h264::{Decoder, hardware::normalize_access_unit}};
use std::{error::Error,fs,path::PathBuf};
fn main()->Result<(),Box<dyn Error>> {
    let root=PathBuf::from(std::env::args_os().nth(1).ok_or("validation directory required")?);
    for count in [1usize,15] {
        let mut decoder=Decoder::new()?;
        let mut normalized=Vec::new();
        let mut maximum=0i16;
        for frame in 0..count {
            let raw=fs::read(root.join(format!("frames-{count}/frame-{frame:03}.au")))?;
            normalize_access_unit(&raw,640,360,frame==0,&mut normalized)?;
            let pixels=decoder.decode(Header{width:640,height:360,format:PixelFormat::Rgb,codec:Codec::H264,payload_len:normalized.len()},&normalized)?;
            assert_eq!(pixels.len(),640*360*3);
            for row in 0..360 {
                for x in 0..640 {
                    let y=1.16438356*((32+20*((x/80+frame)%8)) as f64-16.0);
                    let u=[90.0,110.0,150.0,170.0][(x/160+frame)%4]-128.0;
                    let v=[170.0,150.0,110.0,90.0][(x/160+frame)%4]-128.0;
                    let expected=[y+1.79274107*v,y-0.21324861*u-0.53290933*v,y+2.11240179*u].map(|v|v.round().clamp(0.0,255.0) as i16);
                    let pixel=&pixels[(row*640+x)*3..(row*640+x+1)*3];
                    for c in 0..3 {
                        let error=(i16::from(pixel[c])-expected[c]).abs();
                        maximum=maximum.max(error);
                        assert!(error<=4,"count{count} frame{frame} row{row} x{x}: {pixel:?} vs {expected:?}");
                    }
                }
            }
        }
        println!("frames-{count}: all capture AUs passed unchanged normalize_access_unit and Decoder; all {} RGB pixels checked; maximum channel error {maximum} <= 4",count*640*360);
    }
    Ok(())
}
