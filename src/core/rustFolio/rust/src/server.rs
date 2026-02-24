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
    /// Auth token validated during the TCP handshake.
    /// Generated from /dev/urandom at startup; written (hex-encoded) into
    /// the port file so only the process that spawned us knows the secret.
    token:   [u8; protocol::TOKEN_LEN],
}

impl Server {
    /// Open the database and start the server.
    pub fn open(config: Config) -> Result<Self> {
        let storage = Storage::open(&config.db_path())?;

        // ── Prefix rename (before cache load) ────────────────────────────────
        // If the caller supplied a different id prefix than what is stored in
        // META, atomically rewrite all RECORDS keys, HISTORY composite keys,
        // HISTORY_META keys, and Ref values inside every record dict so that
        // stored data reflects the new prefix before any connection is accepted.
        let stored_prefix = storage.read_id_prefix()?;
        let new_prefix    = config.id_prefix.as_deref().unwrap_or("");

        let stored_str = stored_prefix.as_deref().unwrap_or("");
        if stored_str != new_prefix {
            if !stored_str.is_empty() && !new_prefix.is_empty() {
                // Both sides known and non-empty → full rename.
                tracing::info!(old = %stored_str, new = %new_prefix, "id prefix rename detected");
                storage.rename_prefix(stored_str, new_prefix)?;
                // rename_prefix updates META "idPrefix" in the same transaction.
            } else {
                // New DB, upgrade from a pre-rename-feature build, or prefix
                // cleared — just record the current prefix without renaming.
                storage.write_id_prefix(new_prefix)?;
            }
        }
        // ─────────────────────────────────────────────────────────────────────

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

        // Generate auth token from /dev/urandom.
        // Written (hex-encoded) into the port file; validated during handshake.
        let token = Self::generate_token()?;

        Ok(Server { config, storage, cache, closed: false, token })
    }

    /// Bind TCP listener, signal READY:{port}, serve one connection.
    pub fn run(mut self) -> Result<()> {
        // Bind on ephemeral port (port 0 = OS assigns)
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();

        // Signal ready: write "{port}:{hex-token}" to {dir}/.rust-folio.port.
        // Fantom polls for this file, reads both port and auth token from it.
        // chmod 0600 so only the owning user can read the token.
        let port_file = self.config.dir.join(".rust-folio.port");
        let hex_token: String = self.token.iter().map(|b| format!("{:02x}", b)).collect();
        std::fs::write(&port_file, format!("{}:{}", port, hex_token))?;
        Self::chmod_port_file(&port_file)?;

        tracing::info!(port = port, "listening for connection");

        // Accept exactly one connection (single-connection model)
        let (mut stream, peer) = listener.accept()?;
        tracing::info!(peer = %peer, "client connected");

        // Handshake (version + auth)
        protocol::server_handshake(&mut stream, &self.token)?;
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

            // dispatch returns Vec<Vec<u8>>: one payload per response frame.
            // Most handlers return a single frame; handle_read_all may return many.
            let write_ok = match result {
                Ok(payloads) => {
                    let mut ok = true;
                    for p in payloads {
                        if let Err(e) = protocol::write_response(stream, op, &p) {
                            tracing::error!(err = %e, "write_response failed");
                            ok = false;
                            break;
                        }
                    }
                    ok
                }
                Err(err) => {
                    tracing::warn!(op = op, err = %err, "request error");
                    // Write the error frame to the client.  Only close the
                    // connection if the write itself fails (I/O error), not
                    // because the handler returned a logical error — that is
                    // normal protocol behaviour (e.g. ConcurrentChangeErr).
                    match protocol::write_error(stream, op, &err) {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::error!(err = %e, "write_error failed");
                            false
                        }
                    }
                }
            };
            if !write_ok { break; }

            if op == opcode::CLOSE {
                tracing::info!("close acknowledged, exiting");
                break;
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, op: u16, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut pos = 0usize;
        // Most handlers return a single frame; READ_ALL may return many.
        // Single-frame handlers are wrapped with `.map(|b| vec![b])`.
        match op {
            opcode::CLOSE         => self.handle_close().map(|b| vec![b]),
            opcode::SYNC          => self.handle_sync().map(|b| vec![b]),
            opcode::CUR_VER       => self.handle_cur_ver().map(|b| vec![b]),
            opcode::FLUSH_MODE    => self.handle_flush_mode(payload, &mut pos).map(|b| vec![b]),
            opcode::FLUSH         => self.handle_flush().map(|b| vec![b]),
            opcode::READ_BY_ID    => self.handle_read_by_id(payload, &mut pos).map(|b| vec![b]),
            opcode::READ_BY_IDS   => self.handle_read_by_ids(payload, &mut pos).map(|b| vec![b]),
            opcode::READ_ALL      => self.handle_read_all(payload, &mut pos),
            opcode::READ_COUNT    => self.handle_read_count(payload, &mut pos).map(|b| vec![b]),
            opcode::COMMIT_ALL    => self.handle_commit_all(payload, &mut pos).map(|b| vec![b]),
            opcode::HIS_READ      => self.handle_his_read(payload, &mut pos).map(|b| vec![b]),
            opcode::HIS_WRITE     => self.handle_his_write(payload, &mut pos).map(|b| vec![b]),
            opcode::HIS_STAT      => self.handle_his_stat(payload, &mut pos).map(|b| vec![b]),
            opcode::BACKUP_CREATE => self.handle_backup_create(payload, &mut pos).map(|b| vec![b]),
            opcode::SPEC_UPDATE   => self.handle_spec_update(payload, &mut pos).map(|b| vec![b]),
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
        let abs_id = commit::norm_id(&id_ref.id, &self.config);
        let mut buf = Vec::new();
        match self.cache.get(&abs_id) {
            None => {
                protocol::write_u8(&mut buf, 0x00);
            }
            Some(rec) => {
                protocol::write_u8(&mut buf, 0x01);
                let dict = self.norm_id_dis(&rec.merged);
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
            let abs_id = commit::norm_id(&id_ref.id, &self.config);
            match self.cache.get(&abs_id) {
                None => protocol::write_u8(&mut buf, 0x00),
                Some(rec) => {
                    protocol::write_u8(&mut buf, 0x01);
                    let dict = self.norm_id_dis(&rec.merged);
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
            let enriched_new = r.new_rec.as_ref().map(|d| self.norm_id_dis(d));
            let enriched_old = r.old_rec.as_ref().map(|d| self.norm_id_dis(d));
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

    /// Maximum records per READ_ALL response frame.
    /// At ~2 KB average record size this yields ~2 MB per chunk — well within
    /// the 64 MB per-frame cap while keeping frame count low for typical data sets.
    const CHUNK_SIZE: usize = 1000;

    /// READ_ALL — return all records matching the filter.
    ///
    /// The query snapshot is atomic: all records are collected from RecordCache
    /// in one pass before any serialization begins.  Chunking only affects how
    /// the serialized bytes are split across response frames — there is no
    /// TOCTOU risk between chunks.
    ///
    /// Response frame format: [u8 has_more][u32 count]{dict * count}
    ///   has_more = 0x01 → more frames follow for this request
    ///   has_more = 0x00 → this is the final (or only) frame
    fn handle_read_all(&self, data: &[u8], pos: &mut usize) -> Result<Vec<Vec<u8>>> {
        if self.closed { return Err(FolioError::Shutdown); }
        let filter_str = protocol::read_str_from(data, pos)?;
        let opts_dict  = read_dict(data, pos)?;

        let f    = filter::parse(&filter_str)?;
        let opts = QueryOpts::from_dict(&opts_dict);
        let recs = query::read_all(&f, &opts, &self.cache);

        // Empty result: single final frame with has_more=0, count=0.
        if recs.is_empty() {
            let mut buf = Vec::with_capacity(5);
            buf.push(0x00u8);
            protocol::write_u32(&mut buf, 0);
            return Ok(vec![buf]);
        }

        let chunks: Vec<&[Dict]> = recs.chunks(Self::CHUNK_SIZE).collect();
        let last_idx = chunks.len() - 1;
        let mut payloads = Vec::with_capacity(chunks.len());

        for (i, chunk) in chunks.iter().enumerate() {
            let mut buf = Vec::new();
            buf.push(if i < last_idx { 0x01u8 } else { 0x00u8 }); // has_more
            protocol::write_u32(&mut buf, chunk.len() as u32);
            for rec in *chunk {
                let enriched = self.norm_id_dis(rec);
                write_dict(&mut buf, &enriched);
            }
            payloads.push(buf);
        }
        Ok(payloads)
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

    // ── History handlers ─────────────────────────────────────────────────────

    /// HIS_WRITE — persist a batch of history items for one point.
    ///
    /// Request payload:
    ///   [Ref id] [u32 count] { [i64 ticks] [val_bytes...] }* [Dict opts]
    ///
    /// Response payload:
    ///   [u64 size] [i64 first_ticks] [i64 last_ticks]
    fn handle_his_write(&mut self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }

        let id_ref = read_href(data, pos)?;
        let count  = protocol::read_u32(data, pos)? as usize;

        let mut items: Vec<(i64, Vec<u8>)> = Vec::with_capacity(count);
        for _ in 0..count {
            let ticks = protocol::read_i64(data, pos)?;
            // Read a single val: consume the remainder of this item.
            // write_val serialises a type-byte + optional payload; read_val
            // advances *pos past the complete value.
            let val_start = *pos;
            read_val(data, pos)?;           // advances pos; result discarded (we store raw bytes)
            let val_bytes = data[val_start..*pos].to_vec();
            items.push((ticks, val_bytes));
        }
        // opts dict — reserved for future use
        let _opts = read_dict(data, pos)?;

        let stat = self.storage.his_write(&id_ref.id, &items)?;

        let mut buf = Vec::with_capacity(24);
        protocol::write_u64(&mut buf, stat.size);
        protocol::write_i64(&mut buf, stat.first_ticks);
        protocol::write_i64(&mut buf, stat.last_ticks);
        Ok(buf)
    }

    /// HIS_READ — read history items for one point, with optional span.
    ///
    /// Request payload:
    ///   [Ref id] [u8 mode: 0=all, 1=span] [i64 start_ticks?] [i64 end_ticks?] [Dict opts]
    ///
    /// Response payload:
    ///   [u32 count] { [i64 ticks] [val_bytes...] }*
    fn handle_his_read(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }

        let id_ref    = read_href(data, pos)?;
        let mode      = protocol::read_u8(data, pos)?;
        let span_mode = mode == 0x01;
        let start_ticks = if span_mode { protocol::read_i64(data, pos)? } else { 0 };
        let end_ticks   = if span_mode { protocol::read_i64(data, pos)? } else { 0 };
        let _opts = read_dict(data, pos)?;

        let raw_items = self.storage.his_read(&id_ref.id, span_mode, start_ticks, end_ticks)?;

        let mut buf = Vec::new();
        protocol::write_u32(&mut buf, raw_items.len() as u32);
        for (ticks, val_bytes) in &raw_items {
            protocol::write_i64(&mut buf, *ticks);
            buf.extend_from_slice(val_bytes);
        }
        Ok(buf)
    }

    /// HIS_STAT — return lightweight stats (size, first_ticks, last_ticks).
    ///
    /// Request payload:  [Ref id]
    /// Response payload: [u64 size] [i64 first_ticks] [i64 last_ticks]
    fn handle_his_stat(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }

        let id_ref = read_href(data, pos)?;
        let stat   = self.storage.his_stat(&id_ref.id)?;

        let mut buf = Vec::with_capacity(24);
        protocol::write_u64(&mut buf, stat.size);
        protocol::write_i64(&mut buf, stat.first_ticks);
        protocol::write_i64(&mut buf, stat.last_ticks);
        Ok(buf)
    }

    // ── Backup handlers ──────────────────────────────────────────────────────

    /// BACKUP_CREATE — write a consistent redb snapshot to the given path.
    ///
    /// Request payload:  [str dest_path]
    /// Response payload: empty (success) or error frame
    fn handle_backup_create(&self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        if self.closed { return Err(FolioError::Shutdown); }

        let dest_path_str = protocol::read_str_from(data, pos)?;
        let dest_path     = std::path::Path::new(&dest_path_str);

        self.storage.backup_create(dest_path)?;
        Ok(Vec::new())
    }

    // ── Spec handlers ────────────────────────────────────────────────────────

    /// SPEC_UPDATE — receive the Xeto spec subtype map from Fantom.
    ///
    /// Request payload:
    ///   [u32 entry_count]
    ///   for each entry:
    ///     [str parent_qname]
    ///     [u32 subtype_count]
    ///     for each subtype: [str subtype_qname]
    ///
    /// Response payload: empty (success) or error frame.
    ///
    /// Replaces the entire spec_subtypes map atomically. An entry_count of 0
    /// clears the map (namespace not yet loaded on Fantom side).
    fn handle_spec_update(&mut self, data: &[u8], pos: &mut usize) -> Result<Vec<u8>> {
        use std::collections::HashSet;

        let entry_count = protocol::read_u32(data, pos)? as usize;
        let mut map = std::collections::HashMap::with_capacity(entry_count);

        for _ in 0..entry_count {
            let parent_qname   = protocol::read_str_from(data, pos)?;
            let subtype_count  = protocol::read_u32(data, pos)? as usize;
            let mut subtypes   = HashSet::with_capacity(subtype_count);
            for _ in 0..subtype_count {
                subtypes.insert(protocol::read_str_from(data, pos)?);
            }
            map.insert(parent_qname, subtypes);
        }

        self.cache.spec_subtypes = map;
        Ok(Vec::new())
    }

    /// Generate a cryptographically random auth token from /dev/urandom.
    fn generate_token() -> Result<[u8; protocol::TOKEN_LEN]> {
        use std::io::Read;
        let mut token = [0u8; protocol::TOKEN_LEN];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut token))
            .map_err(|e| crate::error::FolioError::Io(e))?;
        Ok(token)
    }

    /// Restrict the port file to owner-read/write only (chmod 0600).
    /// Prevents other local users from reading the auth token.
    #[cfg(unix)]
    fn chmod_port_file(path: &std::path::Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
        Ok(())
    }

    /// Enrich the id Ref with a dis string derived from the record's "dis" tag.
    /// Full disMacro computation is handled Fantom-side by RustFolioDisMgr.
    fn norm_id_dis(&self, dict: &Dict) -> Dict {
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
