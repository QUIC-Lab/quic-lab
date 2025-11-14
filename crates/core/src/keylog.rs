use std::io::{Result as IoResult, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::rotate::{NewFileHook, RotatingWriter};

const BASE_NAME: &str = "quic-lab.keylog";
const MAX_KEYLOG_BYTES: u64 = 256 * 1024 * 1024;
const FLUSH_EVERY: u32 = 2000;

struct NoHook;
impl NewFileHook for NoHook {}

struct Inner {
    writer: RotatingWriter<NoHook>,
    since_flush: u32,
}

pub struct KeylogSink {
    inner: Mutex<Inner>,
}

static GLOBAL: OnceLock<KeylogSink> = OnceLock::new();

/// Initialise global, rotated keylog sink: `<out_dir>/keylog_files/quic-lab.keylog[.N]`
pub fn init(out_dir: &str, enabled: bool) -> anyhow::Result<()> {
    if !enabled {
        return Ok(());
    }

    let dir = PathBuf::from(out_dir).join("keylog_files");
    std::fs::create_dir_all(&dir)?;
    let writer = RotatingWriter::new(&dir, BASE_NAME, MAX_KEYLOG_BYTES, Some(NoHook))?;

    let sink = KeylogSink {
        inner: Mutex::new(Inner {
            writer,
            since_flush: 0,
        }),
    };

    let _ = GLOBAL.set(sink);
    Ok(())
}

fn append_line(line: &[u8]) -> IoResult<()> {
    if line.is_empty() {
        return Ok(());
    }
    if let Some(sink) = GLOBAL.get() {
        let mut g = sink.inner.lock().unwrap();
        g.writer.write_all(line)?;
        g.since_flush += 1;
        if g.since_flush >= FLUSH_EVERY {
            g.writer.flush()?;
            g.since_flush = 0;
        }
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    GLOBAL.get().is_some()
}

/// Per-connection keylog writer: buffers bytes, splits into full lines, forwards to global sink.
pub struct PerConnKeylog {
    buf: Vec<u8>,
}

impl PerConnKeylog {
    pub fn new() -> Option<Self> {
        if is_enabled() {
            Some(Self {
                buf: Vec::with_capacity(1024),
            })
        } else {
            None
        }
    }

    fn forward_line(&mut self, line: Vec<u8>) {
        // Ignore IO errors; nothing better we can do from here.
        let _ = append_line(&line);
    }
}

impl Write for PerConnKeylog {
    fn write(&mut self, data: &[u8]) -> IoResult<usize> {
        self.buf.extend_from_slice(data);

        // Forward complete lines (ending in '\n') to the global sink.
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            // Include the '\n' in the forwarded line.
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.forward_line(line);
        }

        Ok(data.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        // Only forward complete lines; drop any unfinished tail.
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.forward_line(line);
        }
        self.buf.clear();
        Ok(())
    }
}

impl Drop for PerConnKeylog {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
