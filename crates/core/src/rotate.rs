use std::fs::{self, create_dir_all, rename, File, OpenOptions};
use std::io::{Result as IoResult, Write};
use std::path::{Path, PathBuf};

pub trait NewFileHook: Send {
    /// Called whenever a new active file is created and is empty.
    fn on_new_file(&mut self, _path: &Path, _file: &mut File) -> IoResult<()> {
        Ok(())
    }
}

/// Size-capped writer:
///   base, base.1, base.2, ...
pub struct RotatingWriter<H: NewFileHook> {
    dir: PathBuf,
    base: String,
    max_bytes: u64,

    file: File,
    size: u64,
    next_index: u64,

    hook: Option<H>,
}

impl<H: NewFileHook> RotatingWriter<H> {
    pub fn new<P: AsRef<Path>>(
        dir: P,
        base: &str,
        max_bytes: u64,
        mut hook: Option<H>,
    ) -> IoResult<Self> {
        let dir = dir.as_ref().to_path_buf();
        create_dir_all(&dir)?;

        // discover next index
        let mut max_idx = 0u64;
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(s) = name.strip_prefix(&(base.to_string() + ".")) {
                        if let Ok(i) = s.parse::<u64>() {
                            max_idx = max_idx.max(i);
                        }
                    }
                }
            }
        }
        let next_index = max_idx.saturating_add(1);

        let path = dir.join(base);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut size = file.metadata().map(|m| m.len()).unwrap_or(0);

        // Only call hook if the file is empty (so we can write headers like qlog JSON-SEQ).
        if size == 0 {
            if let Some(h) = hook.as_mut() {
                h.on_new_file(&path, &mut file)?;
                size = file.metadata().map(|m| m.len()).unwrap_or(size);
            }
        }

        Ok(Self {
            dir,
            base: base.into(),
            max_bytes,
            file,
            size,
            next_index,
            hook,
        })
    }

    #[inline]
    fn current_path(&self) -> PathBuf {
        self.dir.join(&self.base)
    }

    fn rotate(&mut self) -> IoResult<()> {
        // close current by dropping
        let _ = &self.file;
        let cur = self.current_path();

        if cur.exists() {
            let numbered = self.dir.join(format!("{}.{}", self.base, self.next_index));
            if numbered.exists() {
                let _ = fs::remove_file(&numbered);
            }
            rename(&cur, &numbered)?;
            self.next_index += 1;
        }

        let mut fresh = OpenOptions::new().create(true).append(true).open(&cur)?;
        if let Some(h) = self.hook.as_mut() {
            h.on_new_file(&cur, &mut fresh)?;
        }
        self.size = fresh.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = fresh;
        Ok(())
    }
}

impl<H: NewFileHook> Write for RotatingWriter<H> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Ensure the whole chunk goes into a single file.
        if self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }

        // Always write the full buffer; avoid partial writes that would
        // split a logical record across rotation boundaries.
        self.file.write_all(buf)?;
        self.size += buf.len() as u64;

        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        self.file.flush()
    }
}
