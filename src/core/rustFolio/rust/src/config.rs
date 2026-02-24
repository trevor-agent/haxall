// config.rs — Server configuration from CLI arguments.
//
// Accepts: --dir <path>  [--flush-mode <mode>]  [--id-prefix <prefix>]  [--name <name>]
// --dir is required; all others are optional with sane defaults.
// No external arg-parsing crate: four flags don't warrant the dep tree.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// Database directory — redb file is created here.
    pub dir: PathBuf,

    /// Flush mode: "fsync" (default) or "nosync".
    pub flush_mode: String,

    /// Optional ref prefix for absolute refs (e.g. "p:proj:r:").
    pub id_prefix: Option<String>,

    /// Database name — defaults to directory name.
    pub name: Option<String>,
}

impl Config {
    /// Parse configuration from `std::env::args()`.
    pub fn parse() -> Self {
        let mut dir:        Option<PathBuf> = None;
        let mut flush_mode: String          = "fsync".to_string();
        let mut id_prefix:  Option<String>  = None;
        let mut name:       Option<String>  = None;

        let mut args = std::env::args().skip(1);
        while let Some(key) = args.next() {
            match key.as_str() {
                "--dir"        => dir        = args.next().map(PathBuf::from),
                "--flush-mode" => flush_mode = args.next().unwrap_or_else(|| "fsync".to_string()),
                "--id-prefix"  => id_prefix  = args.next(),
                "--name"       => name       = args.next(),
                other          => eprintln!("rust-folio: unknown argument: {}", other),
            }
        }

        let dir = dir.unwrap_or_else(|| {
            eprintln!("rust-folio: --dir is required");
            std::process::exit(1);
        });

        Config { dir, flush_mode, id_prefix, name }
    }

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
