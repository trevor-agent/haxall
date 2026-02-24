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

use config::Config;
use tracing::field::{Field, Visit};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, format::Writer};
use tracing_subscriber::registry::LookupSpan;

// ── Log formatting ────────────────────────────────────────────────────────────

/// Custom log formatter matching Haxall's Fantom log format:
///   [HH:MM:SS DD-Mon-YY] [level] [rust-folio] message field=value ...
///
/// Keeps Rust subprocess logs visually consistent with Fantom's own log output
/// and removes the UTC ISO-8601 timestamps that stand out in side-by-side output.
struct FanLogFormat;

impl<S, N> FormatEvent<S, N> for FanLogFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        // [HH:MM:SS DD-Mon-YY] — local time matching Fantom's log timestamp
        let now = chrono::Local::now();
        write!(writer, "[{}] ", now.format("%H:%M:%S %d-%b-%y"))?;

        // [level] — Fantom uses "err" not "error"
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "err",
            tracing::Level::WARN  => "warn",
            tracing::Level::INFO  => "info",
            tracing::Level::DEBUG => "debug",
            tracing::Level::TRACE => "trace",
        };
        write!(writer, "[{}] [rust-folio] ", level)?;

        // Collect fields into a plain String via a custom visitor (no ANSI codes).
        let mut visitor = PlainVisitor::new();
        event.record(&mut visitor);
        write!(writer, "{}", visitor.output)?;

        writeln!(writer)
    }
}

/// Strip ANSI CSI escape sequences (e.g. ESC[3m, ESC[0m) from a string.
/// Defensive utility: tracing-subscriber's DefaultFields formatter injects ANSI
/// styling even when a custom FormatEvent handles the output — this removes it.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip CSI sequence: ESC [ ... <letter>
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_ascii_alphabetic() { break; }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Field visitor that builds a plain `message key=value ...` string.
/// The special "message" field is written first without a key prefix.
/// All other fields follow as `key=value` pairs separated by spaces.
struct PlainVisitor {
    output:  String,
    has_msg: bool,
}

impl PlainVisitor {
    fn new() -> Self {
        PlainVisitor { output: String::new(), has_msg: false }
    }

    fn append_field(&mut self, name: &str, value: String) {
        // Strip ANSI codes that tracing-subscriber may embed via DefaultFields styling.
        let value = strip_ansi(&value);
        if name == "message" {
            self.has_msg = true;
            self.output.push_str(&value);
        } else {
            if self.has_msg { self.output.push(' '); }
            self.output.push_str(&format!("{}={}", name, value));
        }
    }
}

impl Visit for PlainVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.append_field(field.name(), value.to_owned());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // %display fields arrive here via DisplayValue::record → record_debug.
        // format! materialises eagerly (avoids format_args lifetime edge cases).
        self.append_field(field.name(), format!("{:?}", value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.append_field(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.append_field(field.name(), value.to_string());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.append_field(field.name(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.append_field(field.name(), value.to_string());
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    // Tracing to stderr (stdout is reserved for the port-file ready signal).
    tracing_subscriber::fmt()
        .event_format(FanLogFormat)
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
