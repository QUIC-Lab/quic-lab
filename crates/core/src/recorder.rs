use anyhow::Result;
use serde::Serialize;
use std::fs::{self, create_dir_all, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::shard2;

#[derive(Clone)]
pub struct Recorder {
    root: PathBuf,
    ext: &'static str,
}

impl Recorder {
    pub fn new<P: AsRef<Path>>(root: P) -> Result<Self> {
        let root = root.as_ref().join("recorder_files");
        create_dir_all(&root)?;
        Ok(Self { root, ext: "json" })
    }

    /// Compute the final path for a given key (e.g. trace_id).
    pub fn path_for_key(&self, key: &str) -> PathBuf {
        shard2(&self.root, key).join(format!("{key}.{}", self.ext))
    }

    /// Atomically write one JSON file per key, matching qlog’s sharded layout.
    pub fn write_for_key<T: Serialize>(&self, key: &str, value: &T) -> Result<PathBuf> {
        let dir = shard2(&self.root, key);
        create_dir_all(&dir)?;

        let path = dir.join(format!("{key}.{}", self.ext));
        let tmp = dir.join(format!("{key}.tmp-{}", std::process::id()));

        let data = serde_json::to_vec(value)?;
        {
            let f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)?;
            let mut w = BufWriter::new(f);
            w.write_all(&data)?;
            w.flush()?;
        }
        // Single writer per trace_id → rename is fine. Same-dir rename is atomic.
        fs::rename(&tmp, &path)?;

        Ok(path)
    }
}
