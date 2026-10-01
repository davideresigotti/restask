//! Process-wide `tracing` initialization for the `restask` binary (§12.2).

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// Output format of the process log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Compact human lines on stderr; warnings and errors only unless `RUST_LOG` says
    /// otherwise (one-shot commands print their result on stdout).
    Human,
    /// JSON lines on stderr at `info` (the daemon; journald-friendly).
    Json,
}

/// Initializes tracing on stderr. `RUST_LOG` overrides the format's default level.
/// Calling it twice is harmless (the first call wins).
pub fn init(format: LogFormat) {
    let default_level = match format {
        LogFormat::Human => "warn",
        LogFormat::Json => "info",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    let _ = match format {
        LogFormat::Human => builder
            .compact()
            .without_time()
            .with_target(false)
            .with_ansi(std::io::stderr().is_terminal())
            .try_init(),
        LogFormat::Json => builder.json().try_init(),
    };
}
