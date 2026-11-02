use prysm_core::{Config, Frame, PixelFormat, SmoothingSeconds};
use prysm_processor::PrysmProcessor;
fn main() {
    let config = Config { smoothing_seconds: SmoothingSeconds(0.0), remove_black_bars: false, ..Config::default() };
    for value in [16_u8,32,64,96,128,160,192,224,255] {
        let input = Frame::new(vec![value; 32*16*3],32,16,PixelFormat::RGB24).unwrap();
        let edges = PrysmProcessor::new(&config).process_frame(input);
        let linear = edges.top.color_at(0,1).unwrap();
        let ddp = linear.to_srgb();
        assert!(ddp.r.abs_diff(value)<=1 && ddp.g.abs_diff(value)<=1 && ddp.b.abs_diff(value)<=1);
        let actual = ((f32::from(ddp.r)/255.0).powf(2.8)*255.0).round() as u8;
        let gamma22 = ((f32::from(ddp.r)/255.0).powf(2.2)*255.0).round() as u8;
        let srgb = (linear.r*255.0).round() as u8;
        if value==128 {assert_eq!((ddp.r,actual,srgb,gamma22),(128,37,55,56));}
        println!("input={value} prysm_ddp={} wled_gamma28={actual} expected_srgb_pwm={srgb} gamma22_pwm={gamma22}",ddp.r);
    }
}
