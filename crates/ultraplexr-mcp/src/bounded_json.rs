//! Stop encoding at the byte budget, before anything is written to stdout.
use serde_json::Value;
use std::io::{self, Write};

struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP response byte budget exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode(value: &Value, limit: usize, pretty: bool) -> Result<String, &'static str> {
    let mut buffer = Buffer {
        bytes: Vec::with_capacity(4096.min(limit)),
        limit,
    };
    let result = if pretty {
        serde_json::to_writer_pretty(&mut buffer, value)
    } else {
        serde_json::to_writer(&mut buffer, value)
    };
    result.map_err(|_| "MCP response exceeds its serialized byte limit")?;
    String::from_utf8(buffer.bytes).map_err(|_| "Unable to encode MCP response as UTF-8")
}
