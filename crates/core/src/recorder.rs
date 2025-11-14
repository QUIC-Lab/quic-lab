use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::fs::create_dir_all;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::rotate::{NewFileHook, RotatingWriter};

const BASE_NAME: &str = "quic-lab-recorder.jsonl";
const MAX_RECORDER_BYTES: u64 = 128 * 1024 * 1024;
const FLUSH_EVERY: u32 = 2000; // flush every N records

struct NoHook;
impl NewFileHook for NoHook {}

struct Inner {
    writer: RotatingWriter<NoHook>,
    dir: PathBuf,
    base: String,
    since_flush: u32,
}

#[derive(Clone)]
pub struct Recorder {
    // None = disabled (save_recorder_files = false)
    inner: Option<Arc<Mutex<Inner>>>,
}

impl Recorder {
    pub fn new<P: AsRef<Path>>(root: P, save_recorder_files: bool) -> Result<Self> {
        if !save_recorder_files {
            return Ok(Self { inner: None });
        }

        let dir = root.as_ref().join("recorder_files");
        create_dir_all(&dir)?;

        let base = BASE_NAME.to_string();
        let writer = RotatingWriter::new(&dir, &base, MAX_RECORDER_BYTES, Some(NoHook))?;

        Ok(Self {
            inner: Some(Arc::new(Mutex::new(Inner {
                writer,
                dir,
                base,
                since_flush: 0,
            }))),
        })
    }

    /// Append one JSON record for the given key.
    ///
    /// Format (one record per line):
    ///   {"key": "<trace_id>", "value": { ...serialized T... }}
    ///
    /// Returns the current active file path (or empty when disabled).
    pub fn write_for_key<T: Serialize>(&self, key: &str, value: &T) -> Result<PathBuf> {
        let Some(inner) = &self.inner else {
            // recorder disabled via config
            return Ok(PathBuf::new());
        };

        let mut g = inner.lock().unwrap();

        // Build a single JSON object and serialize it into a contiguous buffer.
        let record = json!({
            "key": key,
            "value": value,
        });

        let mut buf = serde_json::to_vec(&record)?;
        buf.push(b'\n');

        // One write for the entire record; rotation can only happen
        // before this call (so the whole record goes into the new file).
        g.writer.write_all(&buf)?;

        g.since_flush += 1;
        if g.since_flush >= FLUSH_EVERY {
            g.writer.flush()?;
            g.since_flush = 0;
        }

        // Active file is always "<dir>/<base>"; rotated files are "<base>.1", ".2", ...
        Ok(g.dir.join(&g.base))
    }
}
