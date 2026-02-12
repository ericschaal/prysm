#[derive(Debug, Clone)]
pub struct Frame<'a> {
    pub buffer: &'a [u8],
    pub width: u32,
    pub height: u32,
}