use std::hash::{DefaultHasher, Hash, Hasher};

pub mod config;
pub mod keylog;
pub mod logging;
pub mod qlog;
pub mod recorder;
pub mod resolver;
pub mod rotate;
pub mod throttle;
pub mod transport;
pub mod types;

fn shard2(base: &std::path::Path, host: &str) -> std::path::PathBuf {
    let mut h = DefaultHasher::new();
    host.hash(&mut h);
    let x = h.finish();
    base.join(format!("{:02x}", (x >> 56) & 0xff))
        .join(format!("{:02x}", (x >> 48) & 0xff))
}
