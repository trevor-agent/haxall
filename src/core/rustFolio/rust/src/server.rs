// server.rs — TCP localhost server, request dispatch.
//
// Design note: The reference design specifies Unix domain sockets, but Fantom's
// Socket class is TCP-only. We use TCP on 127.0.0.1 with an ephemeral port.
// Security is equivalent (loopback-only). Windows fallback is no longer needed.
// The port is output to stdout as "READY:{port}" after bind so the Fantom
// process manager can extract and pass it to the socket client.
//
// Single-threaded request processing matches folio's actor pattern.

use std::net::{TcpListener, TcpStream};
use std::io::Write;
use crate::config::Config;
use crate::error::{FolioError, Result};
use crate::protocol::{self, msg_type, opcode};
use crate::types::*;
use crate::types_ser::*;
use crate::types_de::*;
use crate::record_cache::RecordCache;
use crate::storage::Storage;
use crate::commit;
use crate::filter;
use crate::query::{self, QueryOpts};

pub struct Server {
    config:  Config,
    storage: Storage,
    cache:   RecordCache,
    closed:  bool,
}

impl Server {
    /// Open the database and start the server.
    pub fn open(config: Config) -> Result<Self> {
        let storage = Storage::open(&config.db_path())?;
        let cur_ver = storage.read_cur_ver()?;
        let mut cache = RecordCache::new(cur_ver);
        let records = storage.load_all_records()?;
        cache.load(records);

        tracing::info!(
            dir     = %config.dir.display(),
            records = cache.by_id.len(),
            cur_ver = cur_ver,
            "database opened"
        );

        Ok(Server { config, storage, cache, closed: false })
    }

    /// Bind TCP listener, signal READY:{port}, serve one connection.
    pub fn run(mut self) -> Result<()> {
        // Bind on ephemeral port (port 0 = OS assigns)
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();

        // Signal READY: write port to {dir}/.rust-folio.port, then print "READY" to stdout.
        // Fantom's Process API cannot read from process stdout directly (the out field
        // is an OutStream that receives process output, not an InStream to read from).
        // The port file gives Fantom a reliable way to retrieve the port.
        let port_file = self.config.dir.join(".rust-folio.port");
        std::fs::write(&port_file, port.to_string())?;

        println!("READY:{}", port);
        std::io::stdout().flush()?;

        tracing::info!(port = port, "listening for connection");

        // Accept exactly one connection (single-connection model)
        let (mut stream, peer) = listener.accept()?;
        tracing::info!(peer = %peer, "client connected");

        // Handshake
        protocol::server_handshake(&mut stream)?;
        tracing::info!("handshake complete");

        self.serve_connection(&mut stream)?;

        tracing::info!("connection closed, exiting");

        // Clean up port file
        let port_file = self.config.dir.join(".rust-folio.port");
        let _ = std::fs::remove_file(&port_file);

        Ok(())
    }

    fn serve_connection(&mut self, stream: &mut TcpStream) -> Result<()> {
        loop {
            let (mtype, op, payload) = match protocol::read_message(stream) {
                Ok(msg) => msg,
                Err(FolioError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    tracing::info!("client disconnected");
                    break;
                }
                Err(e) => {
                    tracing::error!(err = %e, "read_message failed");
                    break;
                }
            };

            if mtype != msg_type::REQUEST {
                tracing::warn!(mtype, "unexpected message type");
                continue;
            }

            if self.closed && op != opcode::CLOSE {
                let err = FolioError::Shutdown;
                if let Err(e) = protocol::write_error(stream, op, &err) {
                    tracing::error!(err = %e, "write_error failed");
                }
                continue;
            }

            let result = self.dispatch(op, &payload);

            match result {
                Ok(response_payload) => {
                    if let Err(e) = protocol::write_response(stream, op, &response_payload) {
                        tracing::error!(err = %e, "write_response failed");
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!(op = op, err = %err, "request error");
                    if let Err(e) = protocol::write_error(stream, op, &err) {
                        tracing::error!(err = %e, "write_error failed");
                        break;
                    }
                }
            }

            if op == opcode::CLOSE {
                tracing::info!("close acknowledged, exiting");
                break;
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, op: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let mut pos = 0usize;
        match op {
            opcode::CLOSE       => self.handle_close(),
            opcode::SYNC        => self.handle_sync(),
            opcode::CUR_VER     => self.handle_cur_ver(),
            opcode::FLUSH_MODE  => self.handle_flush_mode(payload, &mut pos),
            opcode::FLUSH       => self.handle_flush(),
            opcode::READ_BY_ID  => self.handle_read_by_id(payload, &mut pos),
            opcode::READ_BY_IDS  => self.handle_read_by_ids(payload, &mut pos),
            opcode::READ_ALL     => self.handle_read_all(payload, &mut pos),
            opcode::READ_COUNT   => self.handle_read_count(payload, &mut pos),
            opcode::COMMIT_ALL   => self.handle_commit_all(payload, &mut pos),
            _ => Err(FolioError::Protocol(format!("Unknown opcode: {:#06x}", op))),
        }
    }

    // ── Handlers ────────────────────────────────────────────────────────────

    fn handle_close(&mut self) -> Result<Vec<u8>> {
        self.closed = true;
        Ok(Vec::new())
    }

    fn handle_sync(&self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn handle_cur_ver(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(8);
        protocol::write_u64(&mut buf, self.cache.cur_ver());
        Ok(buf)
    }

    fn handle_flush_mode(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        let op = protocol::read_u8(data, pos)?;
        if op == 0x01 {
            let _mode = protocol::read_str_from(data, pos)?;
        }
        let mut buf = Vec::new();
        protocol::write_str_to(&mut buf, &self.config.flush_mode);
        Ok(buf)
    }

    fn handle_flush(&self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn handle_read_by_id(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let id_ref = read_href(data, pos)?;
        let abs_id = commit::normalize_id(&id_ref.id, &self.config);
        let mut buf = Vec::new();
        match self.cache.get(&abs_id) {
            None => {
                protocol::write_u8(&mut buf, 0x00);
            }
            Some(rec) => {
                protocol::write_u8(&mut buf, 0x01);
                let dict = self.enrich_id_dis(&rec.merged);
                write_dict(&mut buf, &dict);
            }
        }
        Ok(buf)
    }

    fn handle_read_by_ids(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let count = protocol::read_u32(data, pos)? as usize;
        let mut buf = Vec::new();
        protocol::write_u32(&mut buf, count as u32);
        for _ in 0..count {
            let id_ref = read_href(data, pos)?;
            let abs_id = commit::normalize_id(&id_ref.id, &self.config);
            match self.cache.get(&abs_id) {
                None => protocol::write_u8(&mut buf, 0x00),
                Some(rec) => {
                    protocol::write_u8(&mut buf, 0x01);
                    let dict = self.enrich_id_dis(&rec.merged);
                    write_dict(&mut buf, &dict);
                }
            }
        }
        Ok(buf)
    }

    fn handle_commit_all(&mut self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let count = protocol::read_u32(data, pos)? as usize;
        let mut diffs = Vec::with_capacity(count);
        for _ in 0..count {
            diffs.push(read_diff(data, pos)?);
        }

        let results = commit::commit_all(&mut self.cache, &self.storage, &diffs, &self.config)?;

        let mut buf = Vec::new();
        protocol::write_u32(&mut buf, results.len() as u32);
        for r in &results {
            // Enrich new_rec and old_rec so their id.dis is consistent with readById
            let enriched_new = r.new_rec.as_ref().map(|d| self.enrich_id_dis(d));
            let enriched_old = r.old_rec.as_ref().map(|d| self.enrich_id_dis(d));
            // Result id Ref: derive dis from enriched_new (or old) for consistency
            let enriched_id = if let Some(ref d) = enriched_new {
                d.id().cloned().unwrap_or_else(|| r.id.clone())
            } else {
                r.id.clone()
            };
            write_href(&mut buf, &enriched_id);
            write_opt_datetime(&mut buf, r.old_mod.as_ref());
            write_opt_datetime(&mut buf, r.new_mod.as_ref());
            write_opt_dict(&mut buf, enriched_old.as_ref());
            write_opt_dict(&mut buf, enriched_new.as_ref());
        }
        Ok(buf)
    }

    fn handle_read_all(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let filter_str = protocol::read_str_from(data, pos)?;
        let opts_dict  = read_dict(data, pos)?;

        let f    = filter::parse(&filter_str)?;
        let opts = QueryOpts::from_dict(&opts_dict);
        let recs = query::read_all(&f, &opts, &self.cache);

        let mut buf = Vec::new();
        protocol::write_u32(&mut buf, recs.len() as u32);
        for rec in &recs {
            let enriched = self.enrich_id_dis(rec);
            write_dict(&mut buf, &enriched);
        }
        Ok(buf)
    }

    fn handle_read_count(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let filter_str = protocol::read_str_from(data, pos)?;
        let opts_dict  = read_dict(data, pos)?;

        let f     = filter::parse(&filter_str)?;
        let opts  = QueryOpts::from_dict(&opts_dict);
        let count = query::read_count(&f, &opts, &self.cache);

        let mut buf = Vec::new();
        protocol::write_u64(&mut buf, count);
        Ok(buf)
    }

    /// Enrich the id Ref with a dis string if the record has an explicit "dis" tag.
    /// For M1/M2: only set dis when there is an explicit "dis" string tag.
    /// Full disMacro computation is deferred to M6.
    fn enrich_id_dis(&self, dict: &Dict) -> Dict {
        let mut d = dict.clone();
        if let Some(Val::Ref(id_ref)) = dict.get("id") {
            // Only set dis if the record has an explicit "dis" tag
            let dis_opt = if let Some(Val::Str(s)) = dict.get("dis") {
                Some(s.clone())
            } else {
                None
            };
            if dis_opt != id_ref.dis {
                let mut new_ref = id_ref.clone();
                new_ref.dis = dis_opt;
                d.set("id", Val::Ref(new_ref));
            }
        }
        d
    }
}
