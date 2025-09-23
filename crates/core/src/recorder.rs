use std::fs::{OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::types::ProbeRecord;

/// Very simple JSONL file writer that is safe to call from many threads.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Mutex<BufWriter<std::fs::File>>>,
}

impl Recorder {
    pub fn open<P: AsRef<Path>>(p: P) -> anyhow::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(p)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(BufWriter::new(file))),
        })
    }

    pub fn write(&self, rec: &ProbeRecord) {
        if let Ok(line) = serde_json::to_string(rec) {
            if let Ok(mut w) = self.inner.lock() {
                let _ = w.write_all(line.as_bytes());
                let _ = w.write_all(b"\n");
            }
        }
    }
}
