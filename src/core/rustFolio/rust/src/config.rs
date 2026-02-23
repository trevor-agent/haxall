// config.rs — Server configuration from CLI arguments.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(name = "rust-folio", about = "Rust-backed Haxall Folio storage engine")]
pub struct Config {
    /// Database directory — redb file is created here
    #[arg(long)]
    pub dir: PathBuf,

    /// Flush mode: "fsync" (default) or "nosync"
    #[arg(long, default_value = "fsync")]
    pub flush_mode: String,

    /// Optional ref prefix for absolute refs (e.g. "p:proj:r:")
    #[arg(long)]
    pub id_prefix: Option<String>,

    /// Database name — defaults to directory name
    #[arg(long)]
    pub name: Option<String>,
}

impl Config {
    pub fn db_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("db")
                .to_string()
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.join("folio.redb")
    }
}
