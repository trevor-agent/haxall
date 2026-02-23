// record_cache.rs — In-memory record cache with persistent/transient/merged split.
//
// Mirrors HxFolio's Rec class. The cache is the source of truth for reads.
// redb is the source of truth for persistence.
//
// On startup: load all records from redb into cache (persistent layer only).
// On persistent commit: update both redb and cache.
// On transient commit: update only the in-memory transient layer.
// On close/reopen: transient data is discarded; cache rebuilt from redb.

use std::collections::HashMap;
use crate::types::*;

/// A single record's three-layer state.
pub struct Record {
    /// Persistent tags — stored in redb, survive restart.
    /// Always contains "id" and "mod" tags.
    pub persistent: Dict,

    /// Transient tag overlay — in-memory only, lost on restart.
    /// Empty Dict initially and after restart.
    pub transient: Dict,

    /// Merged view (persistent + transient). Returned by reads.
    pub merged: Dict,

    /// Computed display string for this record's id.
    pub dis: String,

    /// Ticks of last change (persistent or transient) for watch support.
    pub ticks: u64,
}

impl Record {
    pub fn from_persistent(persistent: Dict) -> Self {
        let merged = persistent.clone();
        let dis    = merged.id().map(|r| r.id.clone()).unwrap_or_default();
        Record {
            persistent,
            transient: Dict::new(),
            merged,
            dis,
            ticks: now_ticks(),
        }
    }

    /// Recompute merged from persistent + transient.
    pub fn recompute_merged(&mut self) {
        self.merged = self.persistent.merge(&self.transient);
        self.ticks  = now_ticks();
    }

    pub fn is_trash(&self) -> bool {
        self.merged.is_trash()
    }
}

/// In-memory record cache.
pub struct RecordCache {
    /// Primary index: normalized Ref.id → Record
    pub by_id: HashMap<String, Record>,

    /// Current persistent version (matches redb META "curVer")
    pub cur_ver: u64,
}

impl RecordCache {
    pub fn new(cur_ver: u64) -> Self {
        RecordCache {
            by_id: HashMap::new(),
            cur_ver,
        }
    }

    /// Load all records from storage into cache.
    pub fn load(&mut self, records: Vec<(String, Dict)>) {
        for (id, dict) in records {
            self.by_id.insert(id, Record::from_persistent(dict));
        }
    }

    /// Look up a record by id (returns merged Dict, excludes trash).
    pub fn get(&self, id: &str) -> Option<&Record> {
        let rec = self.by_id.get(id)?;
        if rec.is_trash() { return None; }
        Some(rec)
    }

    /// Look up a record including trash records.
    pub fn get_any(&self, id: &str) -> Option<&Record> {
        self.by_id.get(id)
    }

    /// Apply a persistent commit result to the cache.
    /// `id` — record id
    /// `new_persistent` — new persistent Dict (None = remove)
    pub fn apply_persistent_commit(
        &mut self,
        id: &str,
        new_persistent: Option<Dict>,
    ) {
        match new_persistent {
            None => {
                self.by_id.remove(id);
            }
            Some(dict) => {
                match self.by_id.get_mut(id) {
                    Some(rec) => {
                        rec.persistent = dict;
                        rec.recompute_merged();
                    }
                    None => {
                        self.by_id.insert(id.to_string(), Record::from_persistent(dict));
                    }
                }
            }
        }
    }

    /// Apply a transient commit to the cache (in-memory only).
    pub fn apply_transient_commit(&mut self, id: &str, changes: &Dict) {
        if let Some(rec) = self.by_id.get_mut(id) {
            for (name, val) in &changes.tags {
                if matches!(val, Val::Remove) {
                    rec.transient.remove(name);
                } else {
                    rec.transient.set(name.clone(), val.clone());
                }
            }
            rec.recompute_merged();
        }
    }

    pub fn cur_ver(&self) -> u64 {
        self.cur_ver
    }
}

fn now_ticks() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
