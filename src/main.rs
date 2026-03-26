mod checksum;
mod cli;
mod error;
mod io_engine;
mod metadata;
mod progress;
mod resume;
mod sync;
mod transfer;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();

    if cli.verbose > 1 {
        eprintln!("ssdrsync v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Block size: {} bytes", cli.block_size_bytes());
        eprintln!("Parallel jobs: {}", cli.jobs);
        eprintln!("Direct I/O: {}", !cli.no_direct_io);
        eprintln!("Delta sync: {}", !cli.no_delta);
    }

    match sync::run_sync(&cli).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
