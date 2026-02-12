#[derive(Debug, Clone)]
pub struct Frame {
    pub buffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
