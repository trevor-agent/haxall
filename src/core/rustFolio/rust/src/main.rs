// main.rs — rust-folio server entry point.

mod config;
mod error;
mod types;
mod types_ser;
mod types_de;
mod protocol;
mod storage;
mod record_cache;
mod commit;
mod filter;
mod query;
mod server;

use clap::Parser;
use config::Config;

fn main() {
    // Initialize tracing to stderr (stdout is reserved for "READY" signal)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();
    tracing::info!(dir = %config.dir.display(), "rust-folio starting (TCP mode)");

    match server::Server::open(config) {
        Err(e) => {
            eprintln!("rust-folio: failed to open database: {}", e);
            std::process::exit(1);
        }
        Ok(srv) => {
            if let Err(e) = srv.run() {
                eprintln!("rust-folio: server error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
