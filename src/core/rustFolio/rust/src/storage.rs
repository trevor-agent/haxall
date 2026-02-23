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
    /// sorted by the Fantom side (via FolioUtil.hisWriteCheck).  Each pair
    /// is upserted into the HISTORY table by ticks key.  After the write the
    /// HISTORY_META row for the point is updated atomically with the new
    /// size / first / last stats.
    ///
    /// Returns the updated HisStat for the point.
    pub fn his_write(&self, id: &str, items: &[(i64, Vec<u8>)]) -> Result<HisStat> {
        let tx = self.db.begin_write()?;
        {
            let mut table      = tx.open_table(HISTORY)?;
            let mut meta_table = tx.open_table(HISTORY_META)?;

            for (ticks, val_bytes) in items {
                let key = make_his_key(id, *ticks);
                table.insert(key.as_slice(), val_bytes.as_slice())?;
            }

            // Compute updated stats with a single forward scan of the point's range.
            let prefix      = make_his_prefix(id);
            let prefix_min: Vec<u8> = { let mut p = prefix.clone(); p.extend_from_slice(&[0u8; 8]);    p };
            let prefix_max: Vec<u8> = { let mut p = prefix.clone(); p.extend_from_slice(&[0xFFu8; 8]); p };

            let mut size        = 0u64;
            let mut first_ticks = 0i64;
            let mut last_ticks  = 0i64;
            let mut first_seen  = false;

            for entry in table.range(prefix_min.as_slice()..=prefix_max.as_slice())? {
                let (k, _) = entry?;
                let key_bytes = k.value();
                let t = decode_ticks(&key_bytes[key_bytes.len() - 8..]);
                if !first_seen { first_ticks = t; first_seen = true; }
                last_ticks = t;
                size += 1;
            }

            // Persist the stats row.
            let mut stat_bytes = [0u8; 24];
            stat_bytes[0..8].copy_from_slice(&size.to_be_bytes());
            stat_bytes[8..16].copy_from_slice(&first_ticks.to_be_bytes());
            stat_bytes[16..24].copy_from_slice(&last_ticks.to_be_bytes());
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
}
