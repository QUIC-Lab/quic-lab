use std::fs::{self, create_dir_all, rename, File, OpenOptions};
use std::io::{Result as IoResult, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Hard cap for the active log file before rolling to a new numbered file.
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB
const BASE_NAME: &str = "quic-lab.log";

/// Size-capped writer that creates numbered files:
/// quic-lab.log, quic-lab.log.1, quic-lab.log.2, ...
struct RotatingWriter {
    dir: PathBuf,
    base: String,       // e.g., "quic-lab.log"
    max_bytes: u64,     // constant cap
    file: Option<File>, // current open file
    size: u64,          // current file size
    next_index: u64,    // next suffix to use on rotation
}

impl RotatingWriter {
    fn new<P: AsRef<Path>>(
        dir: P,
        base: &str,
        _ignored_max_bytes: u64, // kept for compat with caller; ignored
        _ignored_backups: usize, // kept for compat with caller; ignored
    ) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        create_dir_all(&dir)?;

        // Determine next index by scanning existing numbered files.
        let mut max_idx = 0u64;
        if let Ok(rd) = fs::read_dir(&dir) {
            for entry in rd.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(s) = name.strip_prefix(&(base.to_string() + ".")) {
                        if let Ok(i) = s.parse::<u64>() {
                            if i > max_idx {
                                max_idx = i;
                            }
                        }
                    }
                }
            }
        }
        let next_index = max_idx.saturating_add(1);

        // Open current log file (create if missing) and record its current size.
        let path = dir.join(base);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);

        Ok(Self {
            dir,
            base: base.into(),
            max_bytes: MAX_LOG_BYTES,
            file: Some(file),
            size,
            next_index,
        })
    }

    #[inline]
    fn current_path(&self) -> PathBuf {
        self.dir.join(&self.base)
    }

    fn rotate(&mut self) -> IoResult<()> {
        // Close current file before rename (Windows requires this).
        if let Some(f) = self.file.take() {
            drop(f);
        }

        let cur = self.current_path();
        // Only attempt rename if current file exists.
        if cur.exists() {
            let numbered = self.dir.join(format!("{}.{}", self.base, self.next_index));
            // If a stale target exists, overwrite it.
            if numbered.exists() {
                let _ = fs::remove_file(&numbered);
            }
            rename(&cur, &numbered)?;
            self.next_index += 1;
        }

        // Open a fresh active file.
        let fresh = OpenOptions::new().create(true).append(true).open(&cur)?;
        self.file = Some(fresh);
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        // Rotate before writing if the new write would exceed the cap.
        if self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let f = self.file.as_mut().expect("log file not open");
        let n = f.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> IoResult<()> {
        if let Some(f) = self.file.as_mut() {
            f.flush()
        } else {
            Ok(())
        }
    }
}

/// Thread-safe wrapper for env_logger sink.
struct ThreadSafeWriter(Mutex<RotatingWriter>);
impl ThreadSafeWriter {
    fn new(inner: RotatingWriter) -> Self {
        Self(Mutex::new(inner))
    }
}
impl Write for ThreadSafeWriter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let mut g = self.0.lock().unwrap();
        g.write(buf)
    }
    fn flush(&mut self) -> IoResult<()> {
        let mut g = self.0.lock().unwrap();
        g.flush()
    }
}

/// Initialize env_logger to write only to <out_dir>/log_files/quic-lab.log with size-based numbering.
/// No console output.
pub fn init_file_logger(out_dir: &str, level: log::LevelFilter) -> anyhow::Result<PathBuf> {
    use env_logger::{Builder, Target};
    use std::io::Write;

    // Build <out_dir>/log_files
    let dir = PathBuf::from(out_dir).join("log_files");
    create_dir_all(&dir)?;

    // The arguments `max_bytes` and `backups` are ignored; we use the constants above.
    let writer = RotatingWriter::new(&dir, BASE_NAME, MAX_LOG_BYTES, 0)?;
    let log_path = dir.join(BASE_NAME);

    let mut b = Builder::new();
    b.format(|buf, record| {
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
        writeln!(
            buf,
            "[{}] {} {}: {}",
            ts,
            record.level(),
            record.target(),
            record.args()
        )
    });
    b.filter_level(level);

    if let Ok(spec) = std::env::var("RUST_LOG") {
        b.parse_filters(&spec);
    } else {
        // keep tquic noisy logs down unless user overrides
        b.parse_filters("tquic=warn,tquic::h3=warn");
    }

    // File-only target, with our numbered-rotation writer
    b.target(Target::Pipe(Box::new(ThreadSafeWriter::new(writer))));
    b.init();

    Ok(log_path)
}
