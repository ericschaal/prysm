use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, BufRead, Read};

/// Longest line, including the newline.
pub const MAX_LINE: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("link I/O failed")]
    Io(#[from] io::Error),
    #[error("line exceeds {MAX_LINE} bytes")]
    Oversized,
    #[error("stream ended inside a line")]
    Truncated,
    #[error("malformed message")]
    Malformed(#[from] serde_json::Error),
}

/// One message as a line, newline included.
///
/// # Errors
/// [`LinkError::Oversized`] if it would exceed [`MAX_LINE`].
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, LinkError> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    if line.len() > MAX_LINE {
        return Err(LinkError::Oversized);
    }
    Ok(line)
}

/// Decode one line, with or without its newline.
///
/// # Errors
/// [`LinkError::Oversized`] or [`LinkError::Malformed`].
pub fn decode<T: DeserializeOwned>(line: &[u8]) -> Result<T, LinkError> {
    if line.len() > MAX_LINE {
        return Err(LinkError::Oversized);
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    Ok(serde_json::from_slice(line)?)
}

/// Reads messages from a byte stream, never buffering more than one line.
#[derive(Debug)]
pub struct LineReader<R> {
    inner: R,
    line: Vec<u8>,
}

impl<R: BufRead> LineReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            line: Vec::with_capacity(256),
        }
    }

    /// The next message; `Ok(None)` at a clean end of stream.
    ///
    /// # Errors
    /// I/O failure, an oversized or malformed line, or a stream that ends
    /// inside a line. The stream is unusable afterwards.
    pub fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, LinkError> {
        self.line.clear();
        let limit = u64::try_from(MAX_LINE + 1).unwrap_or(u64::MAX);
        (&mut self.inner)
            .take(limit)
            .read_until(b'\n', &mut self.line)?;
        match self.line.last() {
            None => Ok(None),
            Some(b'\n') => decode(&self.line).map(Some),
            Some(_) if self.line.len() > MAX_LINE => Err(LinkError::Oversized),
            Some(_) => Err(LinkError::Truncated),
        }
    }
}
