// record_cache.rs — In-memory record cache with persistent/transient/merged split.
//
// FolioRec mirrors HxFolio's FolioRec class. The cache is the source of truth for reads.
// redb is the source of truth for persistence.
//
// On startup: load all records from redb into cache (persistent layer only).
// On persistent commit: update both redb and cache.
// On transient commit: update only the in-memory transient layer.
// On close/reopen: transient data is discarded; cache rebuilt from redb.

use std::collections::{HashMap, HashSet};
use crate::types::*;

/// A single record's three-layer state.
pub struct FolioRec {
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

impl FolioRec {
    pub fn from_persistent(persistent: Dict) -> Self {
        let merged = persistent.clone();
        let dis    = merged.id().map(|r| r.id.clone()).unwrap_or_default();
        FolioRec {
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
    /// Primary index: normalized Ref.id → FolioRec
    pub by_id: HashMap<String, FolioRec>,

    /// Secondary tag-presence index: tag name → set of record ids that have the tag.
    /// Tracks the merged view (persistent + transient). Used to accelerate Has-based
    /// filter queries (e.g. `point and his`) by intersecting candidate sets before
    /// running the full filter evaluator.
    pub tag_index: HashMap<String, HashSet<String>>,

    /// Current persistent version (matches redb META "curVer")
    pub cur_ver: u64,

    /// Xeto spec hierarchy: parent_qname → set of all spec qnames that are-a parent
    /// (including the parent itself). Populated via SPEC_UPDATE (0x0060) RPC.
    /// Used by the IsSpec filter evaluator to resolve spec inheritance without
    /// requiring Xeto knowledge inside the Rust process.
    pub spec_subtypes: HashMap<String, HashSet<String>>,
}

impl RecordCache {
    pub fn new(cur_ver: u64) -> Self {
        RecordCache {
            by_id:         HashMap::new(),
            tag_index:     HashMap::new(),
            spec_subtypes: HashMap::new(),
            cur_ver,
        }
    }

    /// Load all records from storage into cache.
    pub fn load(&mut self, records: Vec<(String, Dict)>) {
        for (id, dict) in records {
            self.index_add(&id, &dict);
            self.by_id.insert(id, FolioRec::from_persistent(dict));
        }
    }

    /// Look up a record by id (returns merged Dict, excludes trash).
    pub fn get(&self, id: &str) -> Option<&FolioRec> {
        let rec = self.by_id.get(id)?;
        if rec.is_trash() { return None; }
        Some(rec)
    }

    /// Look up a record including trash records.
    pub fn get_any(&self, id: &str) -> Option<&FolioRec> {
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
                // Capture old merged keys before removal, then remove from index.
                if let Some(rec) = self.by_id.get(id) {
                    let old_keys: Vec<String> = rec.merged.iter()
                        .map(|(k, _)| k.to_string())
                        .collect();
                    for key in &old_keys {
                        self.index_remove_one(key, id);
                    }
                }
                self.by_id.remove(id);
            }
            Some(dict) => {
                match self.by_id.get_mut(id) {
                    Some(rec) => {
                        // Snapshot old merged keys, apply commit, diff and update index.
                        let old_keys: HashSet<String> = rec.merged.iter()
                            .map(|(k, _)| k.to_string())
                            .collect();
                        rec.persistent = dict;
                        rec.recompute_merged();
                        let new_keys: HashSet<String> = rec.merged.iter()
                            .map(|(k, _)| k.to_string())
                            .collect();
                        self.index_diff(id, &old_keys, &new_keys);
                    }
                    None => {
                        // New record — add all merged tags to index.
                        let rec = FolioRec::from_persistent(dict);
                        self.index_add(id, &rec.merged);
                        self.by_id.insert(id.to_string(), rec);
                    }
                }
            }
        }
    }

    /// Apply a transient commit to the cache (in-memory only).
    pub fn apply_transient_commit(&mut self, id: &str, changes: &Dict) {
        if let Some(rec) = self.by_id.get_mut(id) {
            // Snapshot old merged keys before updating transient layer.
            let old_keys: HashSet<String> = rec.merged.iter()
                .map(|(k, _)| k.to_string())
                .collect();

            for (name, val) in &changes.tags {
                if matches!(val, Val::Remove) {
                    rec.transient.remove(name);
                } else {
                    rec.transient.set(name.clone(), val.clone());
                }
            }
            rec.recompute_merged();

            // Update index only if tag presence changed.
            // Most transient commits (e.g. curVal updates) change values but not
            // key presence — the diff will be empty and this is effectively free.
            let new_keys: HashSet<String> = rec.merged.iter()
                .map(|(k, _)| k.to_string())
                .collect();
            self.index_diff(id, &old_keys, &new_keys);
        }
    }

    pub fn cur_ver(&self) -> u64 {
        self.cur_ver
    }

    // -------------------------------------------------------------------------
    // Index maintenance

    /// Add all tags in `dict` to the index for `id`.
    fn index_add(&mut self, id: &str, dict: &Dict) {
        for (key, _) in dict.iter() {
            self.tag_index
                .entry(key.to_string())
                .or_default()
                .insert(id.to_string());
        }
    }

    /// Remove `id` from the index entry for a single tag key.
    fn index_remove_one(&mut self, key: &str, id: &str) {
        if let Some(set) = self.tag_index.get_mut(key) {
            set.remove(id);
            if set.is_empty() {
                self.tag_index.remove(key);
            }
        }
    }

    /// Update the index based on the diff between old and new merged tag key sets.
    fn index_diff(&mut self, id: &str, old_keys: &HashSet<String>, new_keys: &HashSet<String>) {
        // Tags removed from merged → remove from index
        for key in old_keys.difference(new_keys) {
            self.index_remove_one(key, id);
        }
        // Tags added to merged → add to index
        for key in new_keys.difference(old_keys) {
            self.tag_index
                .entry(key.clone())
                .or_default()
                .insert(id.to_string());
        }
    }
}

fn now_ticks() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
