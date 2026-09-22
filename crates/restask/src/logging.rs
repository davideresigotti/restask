//! Process-wide `tracing` initialization for the `restask` binary.

use tracing_subscriber::EnvFilter;

/// Initializes tracing with an `EnvFilter` built from `RUST_LOG`,
/// defaulting to `info` when unset or invalid.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
