use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::{self, WorkerGuard};
use tracing_log::LogTracer;
use tracing_subscriber::{fmt, EnvFilter};

use crate::rotate::{NewFileHook, RotatingWriter};

const MAX_LOG_BYTES: u64 = 128 * 1024 * 1024; // 64 MiB
const BASE_NAME: &str = "quic-lab.log";

struct NoHook;
impl NewFileHook for NoHook {}

struct ThreadSafeWriter(Mutex<RotatingWriter<NoHook>>);
impl std::io::Write for ThreadSafeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
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

/// Initialise logging to `<out_dir>/log_files/quic-lab.log` with rotation.
pub fn init_file_logger(out_dir: &str, level: log::LevelFilter) -> anyhow::Result<PathBuf> {
    let dir = std::path::PathBuf::from(out_dir).join("log_files");
    std::fs::create_dir_all(&dir)?;

    let writer = ThreadSafeWriter(Mutex::new(RotatingWriter::new(
        &dir,
        BASE_NAME,
        MAX_LOG_BYTES,
        Some(NoHook),
    )?));

    // Non-blocking channel + background worker (default capacity, lossy).
    let (nb, guard) = non_blocking::NonBlockingBuilder::default().finish(writer);
    let _ = LOG_GUARD.set(guard);

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
