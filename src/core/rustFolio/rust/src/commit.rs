// commit.rs — Diff application, validation, and concurrent change detection.
//
// The Rust server performs storage-level validation only:
//   - Existence checks (add: must not exist; update/remove: must exist)
//   - Concurrent change detection (non-force: compare oldMod vs current mod)
//
// Diff pre-validation (FolioUtil.checkDiffs, DiffTagRule, checkTagVal) is
// done on the Fantom side at Diff construction time.

use chrono::Utc;
use chrono_tz::Tz;
use crate::error::{FolioError, Result};
use crate::types::*;
use crate::record_cache::RecordCache;
use crate::storage::Storage;
use crate::config::Config;

/// Apply a batch of Diffs atomically.
/// Returns CommitResult for each diff (in same order as input).
pub fn commit_all(
    cache: &mut RecordCache,
    storage: &Storage,
    diffs: &[Diff],
    config: &Config,
) -> Result<Vec<CommitResult>> {
    // Phase 1: validate all diffs against current cache state.
    // Any validation failure aborts the entire batch.
    validate_diffs(cache, diffs, config)?;

    // Phase 2: compute results and new record states.
    let new_mod = now_datetime();
    let mut results     = Vec::with_capacity(diffs.len());
    let mut persistent_changes: Vec<(String, Option<Dict>)> = Vec::new();

    for diff in diffs {
        let id = norm_id(&diff.id.id, config);
        // Normalize all Ref-valued tags in the changes (relative → absolute),
        // mirroring FolioFlatFile's Etc.mapRefs pass during commit.
        // Pass the cache so dis can be looked up for relative Refs with no dis.
        let changes = norm_refs_in_dict(&diff.changes, config, Some(cache));

        if diff.is_transient() {
            // Transient commit — in-memory only, no redb write.
            let old_rec = cache.get_any(&id).map(|r| r.merged.clone());
            cache.apply_transient_commit(&id, &changes);
            let new_rec = cache.get_any(&id).map(|r| r.merged.clone());
            results.push(CommitResult {
                id:      HRef::new(id.clone()),
                old_mod: old_rec.as_ref().and_then(|d| d.mod_time()).cloned(),
                new_mod: None, // transient commits don't update mod
                old_rec,
                new_rec,
            });
        } else if diff.is_add() {
            // Add — create new record.
            let new_persistent = build_add_dict(&diff.id.id, &changes, &new_mod, config);
            persistent_changes.push((id.clone(), Some(new_persistent.clone())));
            results.push(CommitResult {
                id:      HRef::new(id.clone()),
                old_mod: None,
                new_mod: Some(new_mod.clone()),
                old_rec: None,
                new_rec: Some(new_persistent),
            });
        } else if diff.is_remove() {
            // Remove — delete record.
            let old_rec = cache.get_any(&id).map(|r| r.merged.clone());
            let old_mod = old_rec.as_ref().and_then(|d| d.mod_time()).cloned();
            persistent_changes.push((id.clone(), None));
            results.push(CommitResult {
                id:      HRef::new(id.clone()),
                old_mod,
                new_mod: Some(new_mod.clone()),
                old_rec,
                new_rec: None,
            });
        } else {
            // Update — modify existing record.
            let old_rec   = cache.get_any(&id).map(|r| r.persistent.clone());
            let old_mod   = old_rec.as_ref().and_then(|d| d.mod_time()).cloned();
            let base      = old_rec.clone().unwrap_or_default();
            let mut new_p = base.apply_changes(&changes);
            // Update mod timestamp
            new_p.set("mod", Val::DateTime(new_mod.clone()));
            persistent_changes.push((id.clone(), Some(new_p.clone())));
            // New merged = new persistent + current transient
            let transient = cache.get_any(&id)
                .map(|r| r.transient.clone())
                .unwrap_or_default();
            let new_merged = new_p.merge(&transient);
            results.push(CommitResult {
                id:      HRef::new(id.clone()),
                old_mod,
                new_mod: Some(new_mod.clone()),
                old_rec: cache.get_any(&id).map(|r| r.merged.clone()),
                new_rec: Some(new_merged),
            });
        }
    }

    // Phase 3: persist to redb (atomic — all or nothing for persistent diffs).
    if !persistent_changes.is_empty() {
        let new_ver = storage.commit_records(&persistent_changes, cache.cur_ver)?;
        cache.cur_ver = new_ver;
        // Update cache for persistent changes
        for (id, dict_opt) in &persistent_changes {
            cache.apply_persistent_commit(id, dict_opt.clone());
        }
    }

    Ok(results)
}

/// Validate all diffs — abort on first failure.
fn validate_diffs(cache: &RecordCache, diffs: &[Diff], config: &Config) -> Result<()> {
    for diff in diffs {
        let id = norm_id(&diff.id.id, config);
        let id = id.as_str();

        if diff.is_add() {
            // Record must NOT exist
            if cache.get_any(id).is_some() {
                return Err(FolioError::CommitErr(format!(
                    "Cannot add record that already exists: {}", id
                )));
            }
        } else {
            // Record must exist for update/remove — commit-level error, not UnknownRec
            let rec = cache.get_any(id).ok_or_else(|| {
                FolioError::CommitErr(format!("Record not found: {}", id))
            })?;

            // Concurrent change detection (unless force flag set)
            if !diff.is_force() && !diff.is_transient() {
                if let Some(old_mod) = &diff.old_mod {
                    if let Some(current_mod) = rec.persistent.mod_time() {
                        // Compare by formatted string to avoid timezone issues
                        let old_str = old_mod.to_rfc3339();
                        let cur_str = current_mod.to_rfc3339();
                        if old_str != cur_str {
                            return Err(FolioError::ConcurrentChange(format!(
                                "Concurrent change on record {}: expected mod={}, actual mod={}",
                                id, old_str, cur_str
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Build the initial persistent Dict for an add.
fn build_add_dict(
    id_str: &str,
    changes: &Dict,
    new_mod: &chrono::DateTime<Tz>,
    config: &Config,
) -> Dict {
    // Use norm_id: handles both prefix application and the "null" guard.
    let abs_id = norm_id(id_str, config);

    // Build from changes, skipping Val::Remove entries (they're no-ops on add)
    let mut dict = Dict::default();
    for (name, val) in &changes.tags {
        if !matches!(val, Val::Remove) {
            dict.set(name.clone(), val.clone());
        }
    }
    dict.set("id",  Val::Ref(HRef::new(abs_id)));
    dict.set("mod", Val::DateTime(new_mod.clone()));
    dict
}

/// Normalize an id with the configured prefix (if any).
/// "null" is Ref.nullRef — never prefixed regardless of config.
pub fn norm_id(id: &str, config: &Config) -> String {
    if id == "null" { return id.to_string(); }
    if let Some(prefix) = &config.id_prefix {
        if !id.contains(':') {
            return format!("{}{}", prefix, id);
        }
    }
    id.to_string()
}

/// Normalize all Ref-valued tags in a Dict: relative refs (no colon) get the
/// idPrefix applied.  If the cache is provided and a relative Ref had no dis,
/// we attempt to look up the referenced record's dis tag and copy it — matching
/// FolioFlatFile's round-trip behaviour where id.disVal is set from the record's
/// computed dis string and therefore propagates into stored ref tags.
pub fn norm_refs_in_dict(dict: &Dict, config: &Config, cache: Option<&RecordCache>) -> Dict {
    let mut out = Dict::default();
    for (name, val) in &dict.tags {
        out.set(name.clone(), norm_refs_in_val(val, config, cache));
    }
    out
}

fn norm_refs_in_val(val: &Val, config: &Config, cache: Option<&RecordCache>) -> Val {
    match val {
        Val::Ref(r) => {
            let abs_id = norm_id(&r.id, config);
            // When cache is available (commit path): derive dis from the live record,
            // matching hxFolio normRef semantics — if the referenced record exists,
            // use its dis; if it does not exist, strip dis (ref.noDis).
            // When cache is absent: preserve the existing dis (non-commit contexts).
            let dis = match cache {
                Some(c) => c.get_any(&abs_id)
                    .and_then(|rec| rec.merged.get("dis"))
                    .and_then(|v| if let Val::Str(s) = v { Some(s.clone()) } else { None }),
                None => r.dis.clone(),
            };
            if abs_id == r.id && dis == r.dis {
                val.clone()
            } else {
                Val::Ref(HRef { id: abs_id, dis })
            }
        }
        Val::List(items) => {
            Val::List(items.iter().map(|v| norm_refs_in_val(v, config, cache)).collect())
        }
        Val::Dict(d) => {
            Val::Dict(norm_refs_in_dict(d, config, cache))
        }
        _ => val.clone(),
    }
}

/// Current time as a DateTime<Tz> in UTC.
fn now_datetime() -> chrono::DateTime<Tz> {
    let utc = Utc::now();
    utc.with_timezone(&chrono_tz::UTC)
}
