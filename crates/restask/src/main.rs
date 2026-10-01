//! restask CLI entry point.

use clap::Parser;

use restask::cli::{self, Cli, Command};
use restask::logging::{self, LogFormat};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    logging::init(match cli.command {
        Command::Daemon { once: false } => LogFormat::Json,
        _ => LogFormat::Human,
    });
    match cli::execute(cli).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(cli::exit_code(&error));
        }
    }
}
