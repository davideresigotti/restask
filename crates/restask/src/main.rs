//! Taskres CLI entry point.

mod logging;

use clap::Parser;

/// Taskres: Markdown checkboxes ⇄ VTODO (CalDAV) sync.
#[derive(Parser)]
#[command(name = "restask", version)]
struct Cli;

fn main() {
    logging::init();
    let _cli = Cli::parse();
}
