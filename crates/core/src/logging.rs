/// Initialize env_logger with a sensible default if `RUST_LOG` is unset.
pub fn init_default_logging() {
    if std::env::var_os("RUST_LOG").is_none() {
        // Safety: set_var with static string is fine here.
        unsafe {
            std::env::set_var(
                "RUST_LOG",
                "info,tquic=warn,tquic::h3=warn",
            );
        }
    }
    env_logger::init();
}
