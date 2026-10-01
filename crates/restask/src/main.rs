//! Restask CLI entry point.

mod logging;

use clap::Parser;

use restask::cli::{self, Cli};

#[tokio::main]
async fn main() {
    logging::init();
    let cli = Cli::parse();
    match cli::execute(cli).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(cli::exit_code(&error));
        }
    }
}
