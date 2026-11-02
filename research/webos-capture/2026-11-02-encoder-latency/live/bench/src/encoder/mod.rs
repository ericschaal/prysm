use anyhow::{Context, Result, ensure};
use prysm_webos_protocol::{Codec, Header, PixelFormat, h264};
use std::{collections::BTreeMap, fs::File, io::{Read, Write}, net::{SocketAddr, TcpStream}, os::fd::AsRawFd, time::{Duration, Instant}};
use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};
#[cfg(feature="baseline")]
#[path="baseline/hardware.rs"]
mod hardware;
#[cfg(not(feature="baseline"))]
#[path="current/hardware.rs"]
mod hardware;

fn cpu_ns() -> u128 {
    let mut time = libc::timespec { tv_sec:0, tv_nsec:0 };
    assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) }, 0);
    time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128
}
fn capture_lock() -> Result<File> {
    let file = File::options().read(true).write(true).create(true).truncate(false).open("/tmp/prysm-capture.lock")?;
    ensure!(unsafe {libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)} == 0, "capture lock held: {}", std::io::Error::last_os_error());
    Ok(file)
}
fn metrics(address: SocketAddr) -> Result<BTreeMap<String,(f64,f64)>> {
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut text=String::new();socket.read_to_string(&mut text)?;
    ensure!(text.starts_with("HTTP/1.0 200") || text.starts_with("HTTP/1.1 200"), "metrics request failed");
    let mut result=BTreeMap::new();
    for line in text.lines() {
        let (suffix,is_sum)=if line.starts_with("prysm_operation_duration_seconds_sum{"){("_sum{",true)}else if line.starts_with("prysm_operation_duration_seconds_count{"){("_count{",false)}else{continue};
        let _=suffix;
        let Some((_,tail))=line.split_once("operation=\"") else{continue};
        let name=tail.split('"').next().context("operation name")?;
        let value=line.rsplit_once(' ').context("metric value")?.1.parse::<f64>()?;
        let pair=result.entry(name.to_string()).or_insert((0.0,0.0));
        if is_sum{pair.0+=value}else{pair.1+=value}
    }
    Ok(result)
}
const PALETTE: [[u8;3];4]=[[240,24,24],[24,240,24],[24,24,240],[96,96,96]];
fn rgb(index:usize)->Vec<u8>{
    let mut pixels=Vec::with_capacity(640*360*3);
    for y in 0u32..360 {for x in 0u32..640 {
        let value=if (16..80).contains(&x)&&(16..80).contains(&y){PALETTE[index%4]}else{
            let x=x+index as u32*3;
            let hash=(x.wrapping_mul(0x45d9f3b)^y.wrapping_mul(0x119de1f3)).wrapping_mul(0x45d9f3b);
            let noise=(hash>>24)as u8&15;
            [((x*3+y)%192)as u8+noise,((x+y*2)%192)as u8+noise,(((x/16^y/16)*23)%192)as u8+noise]
        };pixels.extend_from_slice(&value);
    }}pixels
}
fn input(format:PixelFormat,index:usize)->Result<Vec<u8>>{
    let rgb=rgb(index);if format==PixelFormat::Rgb{return Ok(rgb)}
    let mut pixels=vec![0;640*360*3/2];let(y,uv)=pixels.split_at_mut(640*360);
    let mut image=YuvBiPlanarImageMut{y_plane:BufferStoreMut::Borrowed(y),y_stride:640,uv_plane:BufferStoreMut::Borrowed(uv),uv_stride:640,width:640,height:360};
    yuv::rgb_to_yuv_nv12(&mut image,&rgb,640*3,YuvRange::Limited,YuvStandardMatrix::Bt709,YuvConversionMode::Balanced)?;
    if format==PixelFormat::Nv12{return Ok(pixels)}
    let mut output=pixels[..640*360].to_vec();for row in pixels[640*360..].chunks_exact(640){output.extend_from_slice(row);output.extend_from_slice(row)}
    Ok(output)
}
fn validate(rgb:&[u8],previous:usize)->Result<()> {
    ensure!(rgb.len()==640*360*3,"wrong decoded length");
    let expected=PALETTE[previous%4];
    for c in 0..3 {
        let mut sum=0u32;for y in 32..64 {for x in 32..64 {sum+=u32::from(rgb[(y*640+x)*3+c])}}
        let mean=f64::from(sum)/1024.0;
        ensure!((mean-f64::from(expected[c])).abs()<=18.0,"delayed picture mismatch frame={previous} channel={c} expected={} mean={mean}",expected[c]);
    }Ok(())
}
pub fn run()->Result<()> {
    let _lock=capture_lock()?;
    let exporter=prysm_metrics::start(([127,0,0,1],0).into(),prysm_metrics::Role::Capturer)?;
    for format in [PixelFormat::Nv12,PixelFormat::Rgb,PixelFormat::Nv16] {
        let frames=(0..8).map(|i|input(format,i)).collect::<Result<Vec<_>>>()?;
        let input_checksum=frames.iter().flatten().fold(0u64,|sum,byte|sum.wrapping_mul(31).wrapping_add(u64::from(*byte)));
        let header=Header{width:640,height:360,format,codec:Codec::Raw,payload_len:frames[0].len()};
        let new_start=Instant::now();let mut hardware=hardware::Hardware::new(header,30,2_000_000)?;let startup_ms=new_start.elapsed().as_secs_f64()*1000.;
        let mut decoder=h264::Decoder::new()?;let mut output=Vec::new();let mut samples=Vec::new();let mut cpu=0u128;let mut bytes=0;let mut encoded=0;let mut skipped=0;let mut checksum=0u64;
        let mut before=BTreeMap::new();
        for index in 0..45 {
            if index==5 {before=metrics(exporter.address())?;}
            let iteration=Instant::now();let cpu_start=cpu_ns();let start=Instant::now();
            let kind=hardware.encode(header,&frames[index%8],&mut output)?;
            let elapsed=start.elapsed();let cpu_elapsed=cpu_ns()-cpu_start;
            if index>=5 {samples.push(elapsed.as_secs_f64()*1000.);cpu+=cpu_elapsed;}
            if kind.is_some(){
                ensure!(!cfg!(feature="baseline") || index>0,"unexpected first hardware output");
                let rgb=decoder.decode(Header{format:PixelFormat::Rgb,codec:Codec::H264,payload_len:output.len(),..header},&output)?;
                validate(&rgb,if cfg!(feature="baseline"){(index-1)%8}else{index%8})?;
                if index>=5 {encoded+=1;bytes+=output.len();for &byte in &output{checksum=checksum.wrapping_mul(31).wrapping_add(u64::from(byte));}}
            }else{ensure!(cfg!(feature="baseline") && index==0,"unexpected hardware skip");if index>=5{skipped+=1}}
            std::thread::sleep(Duration::from_secs_f64(1./30.).saturating_sub(iteration.elapsed()));
        }
        let after=metrics(exporter.address())?;
        let mut phases=BTreeMap::new();for(name,(sum,count))in after{let(old_sum,old_count)=before.get(&name).copied().unwrap_or_default();if count>old_count{phases.insert(name,(sum-old_sum)/(count-old_count)*1000.);}}
        ensure!(samples.len()==40&&encoded==40&&skipped==0,"incorrect measured frame count");
        let mean=samples.iter().sum::<f64>()/40.;samples.sort_by(f64::total_cmp);
        let teardown=Instant::now();drop(hardware);let teardown_ms=teardown.elapsed().as_secs_f64()*1000.;
        println!("{}",serde_json::json!({"build":if cfg!(feature="baseline"){"baseline"}else{"current"},"format":format!("{format:?}"),"frames":40,"startup_ms":startup_ms,"mean_ms":mean,"p50_ms":samples[19],"p95_ms":samples[37],"cpu_mean_ms":cpu as f64/40./1_000_000.,"bytes":bytes,"encoded":encoded,"skipped":skipped,"checksum":format!("{checksum:016x}"),"input_checksum":format!("{input_checksum:016x}"),"phase_mean_ms":phases,"teardown_ms":teardown_ms}));
    }Ok(())
}
