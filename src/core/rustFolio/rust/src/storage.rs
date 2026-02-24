// storage.rs — redb table definitions and persistent storage operations.
//
// Tables:
//   RECORDS:      &str  → &[u8]   (Ref.id → serialized persistent Dict)
//   META:         &str  → &[u8]   (system key/value pairs)
//   HISTORY:      &[u8] → &[u8]   (composite key: his_key(id,ticks) → val_bytes)
//   HISTORY_META: &str  → &[u8]   (per-point stat row: size+first_ticks+last_ticks)
//
// HISTORY key encoding:
//   [2 bytes: id_len as u16 big-endian]
//   [id_len bytes: point id UTF-8]
//   [8 bytes: (ticks as u64) XOR 0x8000_0000_0000_0000, big-endian]
//
// The XOR bias maps i64 ticks to u64 so that the natural big-endian
// byte order gives correct chronological ordering (i64::MIN sorts first).

use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use crate::error::{FolioError, Result};
use crate::types::*;
use crate::types_ser::*;
use crate::types_de::*;

// ── Prefix rename helpers ─────────────────────────────────────────────────────

/// Replace the leading `old` prefix in `s` with `new`, returning a new String.
/// If `s` does not start with `old`, returns `s` unchanged as a new String.
fn prefix_replace(s: &str, old: &str, new: &str) -> String {
    if s.starts_with(old) {
        format!("{}{}", new, &s[old.len()..])
    } else {
        s.to_string()
    }
}

/// Recursively rename all Ref ids in a Val that start with `old` prefix.
fn rename_val(val: Val, old: &str, new: &str) -> Val {
    match val {
        Val::Ref(r) if r.id.starts_with(old) => {
            Val::Ref(HRef { id: format!("{}{}", new, &r.id[old.len()..]), dis: r.dis })
        }
        Val::List(items) => {
            Val::List(items.into_iter().map(|v| rename_val(v, old, new)).collect())
        }
        Val::Dict(d) => Val::Dict(rename_refs_in_dict(d, old, new)),
        Val::Grid(g) => {
            let new_rows = g.rows.into_iter()
                .map(|row| rename_refs_in_dict(row, old, new))
                .collect();
            Val::Grid(Grid {
                meta: rename_refs_in_dict(g.meta, old, new),
                cols: g.cols,   // column names are plain strings, not Refs
                rows: new_rows,
            })
        }
        other => other,
    }
}

/// Walk all tags in a Dict and rename any Ref values that start with `old`.
fn rename_refs_in_dict(dict: Dict, old: &str, new: &str) -> Dict {
    let tags = dict.tags.into_iter()
        .map(|(name, val)| (name, rename_val(val, old, new)))
        .collect();
    Dict { tags }
}

// ─────────────────────────────────────────────────────────────────────────────

const RECORDS:      TableDefinition<&str,  &[u8]> = TableDefinition::new("records");
const META:         TableDefinition<&str,  &[u8]> = TableDefinition::new("meta");
const HISTORY:      TableDefinition<&[u8], &[u8]> = TableDefinition::new("history");
const HISTORY_META: TableDefinition<&str,  &[u8]> = TableDefinition::new("history_meta");

// ── History key helpers ──────────────────────────────────────────────────────

/// Bias an i64 tick value so that big-endian byte order is chronological.
fn encode_ticks(ticks: i64) -> [u8; 8] {
    let biased = (ticks as u64) ^ 0x8000_0000_0000_0000u64;
    biased.to_be_bytes()
}

fn decode_ticks(b: &[u8]) -> i64 {
    let biased = u64::from_be_bytes(b[..8].try_into().unwrap());
    (biased ^ 0x8000_0000_0000_0000u64) as i64
}

/// Build the point-id prefix portion of a HISTORY key.
fn make_his_prefix(id: &str) -> Vec<u8> {
    let id_bytes = id.as_bytes();
    let mut v = Vec::with_capacity(2 + id_bytes.len());
    v.extend_from_slice(&(id_bytes.len() as u16).to_be_bytes());
    v.extend_from_slice(id_bytes);
    v
}

/// Build a full HISTORY key for a given point id + ticks value.
fn make_his_key(id: &str, ticks: i64) -> Vec<u8> {
    let mut k = make_his_prefix(id);
    k.extend_from_slice(&encode_ticks(ticks));
    k
}

// ── HisStat ─────────────────────────────────────────────────────────────────

/// Lightweight per-point history statistics returned by his_stat / his_write.
#[derive(Debug, Clone)]
pub struct HisStat {
    /// Number of items stored for this point.
    pub size:        u64,
    /// Ticks of the earliest item (meaningful only when size > 0).
    pub first_ticks: i64,
    /// Ticks of the latest item (meaningful only when size > 0).
    pub last_ticks:  i64,
}

impl HisStat {
    pub fn empty() -> Self { HisStat { size: 0, first_ticks: 0, last_ticks: 0 } }
    pub fn is_empty(&self) -> bool { self.size == 0 }
}

// ── Storage ──────────────────────────────────────────────────────────────────

pub struct Storage {
    pub db: Database,
}

impl Storage {
    /// Open or create the redb database.
    pub fn open(path: &Path) -> Result<Self> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;
        // Initialize all tables (creates them if they do not exist)
        let tx = db.begin_write()?;
        tx.open_table(RECORDS)?;
        tx.open_table(META)?;
        tx.open_table(HISTORY)?;
        tx.open_table(HISTORY_META)?;
        tx.commit()?;
        Ok(Storage { db })
    }

    /// Read curVer from META table.
    pub fn read_cur_ver(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(META)?;
        match table.get("curVer")? {
            None       => Ok(0),
            Some(v)    => {
                let bytes = v.value();
                if bytes.len() < 8 {
                    return Ok(0);
                }
                Ok(u64::from_be_bytes(bytes[..8].try_into().unwrap()))
            }
        }
    }

    /// Write curVer to META table (within a write transaction).
    pub fn write_cur_ver_tx(tx: &redb::WriteTransaction, ver: u64) -> Result<()> {
        let mut table = tx.open_table(META)?;
        table.insert("curVer", ver.to_be_bytes().as_ref())?;
        Ok(())
    }

    /// Load all records from RECORDS table as (id, Dict) pairs.
    pub fn load_all_records(&self) -> Result<Vec<(String, Dict)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let mut records = Vec::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let id = k.value().to_string();
            let bytes = v.value();
            let mut pos = 0usize;
            let dict = read_dict(bytes, &mut pos)?;
            records.push((id, dict));
        }
        Ok(records)
    }

    // ── History operations ───────────────────────────────────────────────────

    /// Write a batch of history items for a single point.
    ///
    /// `items` is a list of `(ticks, val_bytes)` pairs already validated and
    /// sorted ascending by the Fantom side (via FolioUtil.hisWriteCheck).  Each
    /// pair is upserted into the HISTORY table by ticks key.  The HISTORY_META
    /// stat row is updated incrementally — O(batch) rather than O(n_total):
    ///
    ///   - size:  += count of items whose key did not already exist (no his_delete,
    ///             so size is monotonically non-decreasing).
    ///   - first: min(existing.first, items[0].ticks)   — monotonically non-increasing.
    ///   - last:  max(existing.last,  items[last].ticks) — monotonically non-decreasing.
    ///
    /// All three properties are monotonic and commutative, making the incremental
    /// update always correct without a range scan.
    ///
    /// Returns the updated HisStat for the point.
    pub fn his_write(&self, id: &str, items: &[(i64, Vec<u8>)]) -> Result<HisStat> {
        if items.is_empty() {
            return self.his_stat(id);
        }
        let tx = self.db.begin_write()?;
        {
            let mut table      = tx.open_table(HISTORY)?;
            let mut meta_table = tx.open_table(HISTORY_META)?;

            // Read existing stat row (O(1)) to seed the incremental update.
            let existing: Option<HisStat> = match meta_table.get(id)? {
                None => None,
                Some(v) => {
                    let b = v.value();
                    if b.len() < 24 { None } else {
                        Some(HisStat {
                            size:        u64::from_be_bytes(b[0..8].try_into().unwrap()),
                            first_ticks: i64::from_be_bytes(b[8..16].try_into().unwrap()),
                            last_ticks:  i64::from_be_bytes(b[16..24].try_into().unwrap()),
                        })
                    }
                }
            };

            // Upsert items.  Count only new keys so size_delta reflects net additions.
            // table.get() on a WriteTransaction table reflects in-progress inserts, so
            // items with duplicate ticks within the same batch are counted only once.
            let mut size_delta = 0u64;
            for (ticks, val_bytes) in items {
                let key = make_his_key(id, *ticks);
                if table.get(key.as_slice())?.is_none() { size_delta += 1; }
                table.insert(key.as_slice(), val_bytes.as_slice())?;
            }

            // Incremental stat: items are pre-sorted ascending, so items[0] is the
            // batch minimum and items[last] is the batch maximum.
            let batch_first = items[0].0;
            let batch_last  = items[items.len() - 1].0;
            let new_stat = match existing {
                None    => HisStat {
                    size:        size_delta,
                    first_ticks: batch_first,
                    last_ticks:  batch_last,
                },
                Some(s) => HisStat {
                    size:        s.size + size_delta,
                    first_ticks: s.first_ticks.min(batch_first),
                    last_ticks:  s.last_ticks.max(batch_last),
                },
            };

            // Persist updated stat row atomically with the history inserts.
            let mut stat_bytes = [0u8; 24];
            stat_bytes[0..8].copy_from_slice(&new_stat.size.to_be_bytes());
            stat_bytes[8..16].copy_from_slice(&new_stat.first_ticks.to_be_bytes());
            stat_bytes[16..24].copy_from_slice(&new_stat.last_ticks.to_be_bytes());
            meta_table.insert(id, stat_bytes.as_slice())?;
        }
        tx.commit()?;

        self.his_stat(id)
    }

    /// Read history items for a point, optionally filtered to a span.
    ///
    /// When `span_mode` is false, all items are returned.
    ///
    /// When `span_mode` is true the SkySpark boundary semantics are applied:
    ///   - 0 or 1 item strictly before `start_ticks`  (the trailing pre-span value)
    ///   - all items in `[start_ticks, end_ticks)`
    ///   - 0..2 items at or after `end_ticks`          (post-span boundary items)
    ///
    /// All items are returned in chronological order as `(ticks, val_bytes)`.
    pub fn his_read(
        &self,
        id: &str,
        span_mode:   bool,
        start_ticks: i64,
        end_ticks:   i64,
    ) -> Result<Vec<(i64, Vec<u8>)>> {
        let tx    = self.db.begin_read()?;
        let table = tx.open_table(HISTORY)?;

        let prefix: Vec<u8>     = make_his_prefix(id);
        let prefix_min: Vec<u8> = { let mut p = prefix.clone(); p.extend_from_slice(&[0u8; 8]);    p };
        let prefix_max: Vec<u8> = { let mut p = prefix.clone(); p.extend_from_slice(&[0xFFu8; 8]); p };

        if !span_mode {
            // Return all items for this point.
            let mut result = Vec::new();
            for entry in table.range(prefix_min.as_slice()..=prefix_max.as_slice())? {
                let (k, v) = entry?;
                let key_bytes = k.value();
                let ticks = decode_ticks(&key_bytes[key_bytes.len() - 8..]);
                result.push((ticks, v.value().to_vec()));
            }
            return Ok(result);
        }

        // Span mode: 1-before + [start,end) + up-to-2-after.
        let span_start_key: Vec<u8> = make_his_key(id, start_ticks);
        let span_end_key:   Vec<u8> = make_his_key(id, end_ticks);

        // 1. At most 1 item strictly before start_ticks (reverse seek).
        let maybe_before = table
            .range(prefix_min.as_slice()..span_start_key.as_slice())?
            .rev()
            .next()
            .transpose()?
            .map(|(k, v)| {
                let key_bytes = k.value();
                let ticks = decode_ticks(&key_bytes[key_bytes.len() - 8..]);
                (ticks, v.value().to_vec())
            });

        // 2. Items in [start_ticks, end_ticks).
        let mut in_span: Vec<(i64, Vec<u8>)> = Vec::new();
        for entry in table.range(span_start_key.as_slice()..span_end_key.as_slice())? {
            let (k, v) = entry?;
            let key_bytes = k.value();
            let ticks = decode_ticks(&key_bytes[key_bytes.len() - 8..]);
            in_span.push((ticks, v.value().to_vec()));
        }

        // 3. Up to 2 items at or after end_ticks (bounded to this point's prefix).
        let mut after_items: Vec<(i64, Vec<u8>)> = Vec::new();
        for entry in table.range(span_end_key.as_slice()..=prefix_max.as_slice())? {
            if after_items.len() >= 2 { break; }
            let (k, v) = entry?;
            let key_bytes = k.value();
            let ticks = decode_ticks(&key_bytes[key_bytes.len() - 8..]);
            after_items.push((ticks, v.value().to_vec()));
        }

        // Combine in chronological order.
        let mut result = Vec::with_capacity(
            maybe_before.is_some() as usize + in_span.len() + after_items.len()
        );
        if let Some(b) = maybe_before { result.push(b); }
        result.extend(in_span);
        result.extend(after_items);
        Ok(result)
    }

    /// Return lightweight stats for a point (size, first_ticks, last_ticks).
    /// Returns HisStat::empty() if no history exists for the point.
    pub fn his_stat(&self, id: &str) -> Result<HisStat> {
        let tx    = self.db.begin_read()?;
        let table = tx.open_table(HISTORY_META)?;
        match table.get(id)? {
            None => Ok(HisStat::empty()),
            Some(v) => {
                let bytes = v.value();
                if bytes.len() < 24 { return Ok(HisStat::empty()); }
                let size        = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
                let first_ticks = i64::from_be_bytes(bytes[8..16].try_into().unwrap());
                let last_ticks  = i64::from_be_bytes(bytes[16..24].try_into().unwrap());
                Ok(HisStat { size, first_ticks, last_ticks })
            }
        }
    }

    // ── Prefix rename ────────────────────────────────────────────────────────

    /// Read the stored id prefix from META, if any.
    pub fn read_id_prefix(&self) -> Result<Option<String>> {
        let tx    = self.db.begin_read()?;
        let table = tx.open_table(META)?;
        match table.get("idPrefix")? {
            None    => Ok(None),
            Some(v) => {
                let s = String::from_utf8(v.value().to_vec())
                    .map_err(|_| FolioError::Protocol("invalid utf8 in META idPrefix".into()))?;
                if s.is_empty() { Ok(None) } else { Ok(Some(s)) }
            }
        }
    }

    /// Write the id prefix to META (standalone transaction).
    pub fn write_id_prefix(&self, prefix: &str) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(META)?;
            table.insert("idPrefix", prefix.as_bytes())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Atomically rename all stored data from `old` prefix to `new` prefix.
    ///
    /// Rewrites:
    ///   - RECORDS table keys (record ids) and all Ref values inside each dict
    ///   - HISTORY composite keys — the `[u16 id_len][id_bytes]` prefix portion
    ///     is rebuilt with the new id length (handles variable-length prefix changes)
    ///   - HISTORY_META keys (point ids)
    ///   - META "idPrefix" entry
    ///
    /// All changes are committed in a single write transaction — either all
    /// tables are renamed atomically or none are (crash-safe).
    pub fn rename_prefix(&self, old: &str, new: &str) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            // ── RECORDS ──────────────────────────────────────────────────────
            let mut rec_table = tx.open_table(RECORDS)?;

            // Collect all entries before mutating (cannot mutate while iterating).
            let all_records: Vec<(String, Vec<u8>)> = {
                let mut v = Vec::new();
                for entry in rec_table.iter()? {
                    let (k, val) = entry?;
                    v.push((k.value().to_string(), val.value().to_vec()));
                }
                v
            };

            for (old_id, bytes) in &all_records {
                let new_id = prefix_replace(old_id, old, new);
                let mut pos = 0usize;
                let dict     = read_dict(bytes, &mut pos)?;
                let new_dict = rename_refs_in_dict(dict, old, new);
                let mut new_bytes = Vec::new();
                write_dict(&mut new_bytes, &new_dict);

                if new_id != *old_id {
                    rec_table.remove(old_id.as_str())?;
                    rec_table.insert(new_id.as_str(), new_bytes.as_slice())?;
                } else if new_bytes != *bytes {
                    // Same key but Ref values inside changed (external-prefix record
                    // whose tags reference renamed internal records).
                    rec_table.insert(old_id.as_str(), new_bytes.as_slice())?;
                }
            }

            // ── HISTORY composite keys ────────────────────────────────────────
            // Key layout: [u16 id_len][id_bytes][8 ticks_bytes]
            // Rename: extract id, if starts with old prefix rebuild key with new id
            // (u16 id_len recalculated — handles variable-length prefix changes).
            let mut his_table = tx.open_table(HISTORY)?;

            let his_entries: Vec<(Vec<u8>, Vec<u8>)> = {
                let mut v = Vec::new();
                for entry in his_table.iter()? {
                    let (k, val) = entry?;
                    let key = k.value().to_vec();
                    if key.len() >= 2 {
                        let id_len = u16::from_be_bytes([key[0], key[1]]) as usize;
                        if key.len() >= 2 + id_len {
                            let id_bytes = &key[2..2 + id_len];
                            if id_bytes.starts_with(old.as_bytes()) {
                                v.push((key, val.value().to_vec()));
                            }
                        }
                    }
                }
                v
            };

            for (old_key, val_bytes) in &his_entries {
                let old_id_len  = u16::from_be_bytes([old_key[0], old_key[1]]) as usize;
                let old_id_str  = std::str::from_utf8(&old_key[2..2 + old_id_len])
                    .map_err(|_| FolioError::Protocol("invalid utf8 in HISTORY key".into()))?;
                let ticks_bytes = &old_key[2 + old_id_len..]; // always 8 bytes

                let new_id      = prefix_replace(old_id_str, old, new);
                let new_id_bytes = new_id.as_bytes();

                // Rebuild composite key with corrected u16 id_len.
                let mut new_key = Vec::with_capacity(2 + new_id_bytes.len() + ticks_bytes.len());
                new_key.extend_from_slice(&(new_id_bytes.len() as u16).to_be_bytes());
                new_key.extend_from_slice(new_id_bytes);
                new_key.extend_from_slice(ticks_bytes);

                his_table.remove(old_key.as_slice())?;
                his_table.insert(new_key.as_slice(), val_bytes.as_slice())?;
            }

            // ── HISTORY_META ──────────────────────────────────────────────────
            let mut his_meta_table = tx.open_table(HISTORY_META)?;

            let his_meta_entries: Vec<(String, Vec<u8>)> = {
                let mut v = Vec::new();
                for entry in his_meta_table.iter()? {
                    let (k, val) = entry?;
                    if k.value().starts_with(old) {
                        v.push((k.value().to_string(), val.value().to_vec()));
                    }
                }
                v
            };

            for (old_id, stat_bytes) in &his_meta_entries {
                let new_id = prefix_replace(old_id, old, new);
                his_meta_table.remove(old_id.as_str())?;
                his_meta_table.insert(new_id.as_str(), stat_bytes.as_slice())?;
            }

            // ── META idPrefix ─────────────────────────────────────────────────
            let mut meta_table = tx.open_table(META)?;
            meta_table.insert("idPrefix", new.as_bytes())?;
        }
        tx.commit()?;
        tracing::info!(old = %old, new = %new, "id prefix rename complete");
        Ok(())
    }

    // ── Record operations ────────────────────────────────────────────────────

    /// Write a batch of persistent record changes atomically.
    /// `changes` is a list of (id, Option<Dict>):
    ///   - Some(dict) = upsert
    ///   - None = delete
    /// Returns the new curVer.
    pub fn commit_records(
        &self,
        changes: &[(String, Option<Dict>)],
        old_ver: u64,
    ) -> Result<u64> {
        let new_ver = old_ver + 1;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(RECORDS)?;
            for (id, dict_opt) in changes {
                match dict_opt {
                    Some(dict) => {
                        let mut buf = Vec::new();
                        write_dict(&mut buf, dict);
                        table.insert(id.as_str(), buf.as_slice())?;
                    }
                    None => {
                        table.remove(id.as_str())?;
                    }
                }
            }
        }
        Self::write_cur_ver_tx(&tx, new_ver)?;
        tx.commit()?;
        Ok(new_ver)
    }

    // ── Backup ───────────────────────────────────────────────────────────────

    /// Create a consistent point-in-time backup of all tables at `dest_path`.
    ///
    /// Opens a read transaction on the live database (pinning the MVCC snapshot),
    /// then iterates RECORDS, META, HISTORY, and HISTORY_META tables and writes
    /// all key-value pairs into a new redb database at `dest_path`.  The result
    /// is a valid, self-contained redb file that can be opened with Storage::open.
    ///
    /// Any table that does not yet exist (empty database) is silently skipped.
    pub fn backup_create(&self, dest_path: &Path) -> Result<()> {
        // Pin the MVCC snapshot for the duration.
        let rtxn = self.db.begin_read()?;

        // Create destination database (overwrite if it exists).
        if dest_path.exists() {
            std::fs::remove_file(dest_path)?;
        }
        let backup_db = Database::create(dest_path)?;
        let wtxn = backup_db.begin_write()?;

        // Copy RECORDS (str → bytes).
        match rtxn.open_table(RECORDS) {
            Ok(src) => {
                let mut dst = wtxn.open_table(RECORDS)?;
                for entry in src.iter()? {
                    let (k, v) = entry?;
                    dst.insert(k.value(), v.value())?;
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(FolioError::from(e)),
        }

        // Copy META (str → bytes).
        match rtxn.open_table(META) {
            Ok(src) => {
                let mut dst = wtxn.open_table(META)?;
                for entry in src.iter()? {
                    let (k, v) = entry?;
                    dst.insert(k.value(), v.value())?;
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(FolioError::from(e)),
        }

        // Copy HISTORY (bytes → bytes).
        match rtxn.open_table(HISTORY) {
            Ok(src) => {
                let mut dst = wtxn.open_table(HISTORY)?;
                for entry in src.iter()? {
                    let (k, v) = entry?;
                    dst.insert(k.value(), v.value())?;
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(FolioError::from(e)),
        }

        // Copy HISTORY_META (str → bytes).
        match rtxn.open_table(HISTORY_META) {
            Ok(src) => {
                let mut dst = wtxn.open_table(HISTORY_META)?;
                for entry in src.iter()? {
                    let (k, v) = entry?;
                    dst.insert(k.value(), v.value())?;
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => return Err(FolioError::from(e)),
        }

        wtxn.commit()?;
        drop(rtxn); // Release the MVCC snapshot
        Ok(())
    }
}
