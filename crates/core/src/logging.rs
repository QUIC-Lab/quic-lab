use std::fs::{self, create_dir_all, rename, File, OpenOptions};
use std::io::{Result as IoResult, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::{self, WorkerGuard};
use tracing_log::LogTracer;
use tracing_subscriber::{fmt, EnvFilter};

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
    fn new<P: AsRef<Path>>(dir: P, base: &str) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        create_dir_all(&dir)?;

        // Determine next index by scanning existing numbered files.
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
        if let Some(f) = self.file.take() {
            drop(f);
        }
        let cur = self.current_path();
        if cur.exists() {
            let numbered = self.dir.join(format!("{}.{}", self.base, self.next_index));
            // If a stale target exists, overwrite it.
            if numbered.exists() {
                let _ = fs::remove_file(&numbered);
            }
            rename(&cur, &numbered)?;
            self.next_index += 1;
        }
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

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

fn map_level(l: log::LevelFilter) -> tracing_subscriber::filter::LevelFilter {
    use log::LevelFilter as L;
    use tracing_subscriber::filter::LevelFilter as T;
    match l {
        L::Off => T::OFF,
        L::Error => T::ERROR,
        L::Warn => T::WARN,
        L::Info => T::INFO,
        L::Debug => T::DEBUG,
        L::Trace => T::TRACE,
    }
}

/// Initialise logging to `<out_dir>/log_files/quic-lab.log` with rotation and a non-blocking worker.
/// Captures `log::*` macros. Returns the active file path.
pub fn init_file_logger(out_dir: &str, level: log::LevelFilter) -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(out_dir).join("log_files");
    create_dir_all(&dir)?;

    let writer = ThreadSafeWriter::new(RotatingWriter::new(&dir, BASE_NAME)?);

    // Non-blocking channel + background worker (default capacity, lossy).
    let (nb, guard) = non_blocking::NonBlockingBuilder::default().finish(writer);
    let _ = LOG_GUARD.set(guard); // ignore if already set

    // Forward `log` crate records into `tracing`. Ignore if already set.
    let _ = LogTracer::init();

    // Build filter: prefer RUST_LOG, else provided level plus quieting for deps.
    let env = std::env::var("RUST_LOG").ok();
    let base = map_level(level);
    let filter = match env {
        Some(spec) => EnvFilter::new(spec),
        None => EnvFilter::default()
            .add_directive(base.into())
            .add_directive("tquic=warn".parse()?)
            .add_directive("tquic::h3=warn".parse()?),
    };

    let timer = tracing_subscriber::fmt::time::UtcTime::rfc_3339();

    // Do not panic if another global subscriber is already installed.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(nb)
        .with_timer(timer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .event_format(fmt::format().compact())
        .try_init();

    Ok(dir.join(BASE_NAME))
}
