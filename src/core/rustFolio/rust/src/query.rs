// query.rs — ReadAll implementation with opts support.
//
// Opts dict keys (matches Fantom folio opts):
//   trash: Marker  — include trash records (default: exclude)
//   limit: Number  — max records to return
//   sort:  Marker  — sort results by dis (display string)

use std::collections::HashSet;
use crate::types::{Dict, Val};
use crate::filter::{Filter, matches};
use crate::record_cache::RecordCache;

/// Query options parsed from a Haystack opts Dict.
pub struct QueryOpts {
    pub include_trash: bool,
    pub limit:         Option<usize>,
    pub sort:          bool,
}

impl QueryOpts {
    pub fn from_dict(opts: &Dict) -> Self {
        let include_trash = matches!(opts.get("trash"), Some(Val::Marker));
        let limit = match opts.get("limit") {
            Some(Val::Number(n, _)) => Some(*n as usize),
            _ => None,
        };
        let sort = matches!(opts.get("sort"), Some(Val::Marker));
        QueryOpts { include_trash, limit, sort }
    }

    pub fn default() -> Self {
        QueryOpts { include_trash: false, limit: None, sort: false }
    }
}

/// Execute a filter query and return matching Dicts.
pub fn read_all(
    filter: &Filter,
    opts:   &QueryOpts,
    cache:  &RecordCache,
) -> Vec<Dict> {
    let mut results: Vec<Dict> = Vec::new();

    // Use tag index to compute a candidate set when the filter leading AND
    // chain contains at least one simple Has(single_tag) term. Fall back to
    // full scan for OR roots, multi-segment paths, and non-Has-leading filters.
    let index_keys = extract_index_keys(filter);

    if index_keys.is_empty() {
        // Full scan (existing behavior)
        for (_id, rec) in &cache.by_id {
            if !opts.include_trash && rec.is_trash() { continue; }
            if !matches(filter, &rec.merged, cache) { continue; }
            results.push(rec.merged.clone());
            if let Some(lim) = opts.limit {
                if results.len() >= lim { break; }
            }
        }
    } else {
        // Intersect index sets to get candidate ids, then apply full filter.
        if let Some(candidates) = intersect_index(&index_keys, cache) {
            for id in &candidates {
                if let Some(rec) = cache.by_id.get(id.as_str()) {
                    if !opts.include_trash && rec.is_trash() { continue; }
                    if !matches(filter, &rec.merged, cache) { continue; }
                    results.push(rec.merged.clone());
                    if let Some(lim) = opts.limit {
                        if results.len() >= lim { break; }
                    }
                }
            }
        }
        // If any index key has no entries, intersect is empty — zero results.
    }

    // Sort by dis if requested
    if opts.sort {
        results.sort_by(|a, b| {
            let da = dict_dis(a);
            let db = dict_dis(b);
            da.cmp(&db)
        });
    }

    results
}

/// Count records matching a filter (no materialization).
pub fn read_count(
    filter: &Filter,
    opts:   &QueryOpts,
    cache:  &RecordCache,
) -> u64 {
    let mut count: u64 = 0;

    let index_keys = extract_index_keys(filter);

    if index_keys.is_empty() {
        for (_id, rec) in &cache.by_id {
            if !opts.include_trash && rec.is_trash() { continue; }
            if matches(filter, &rec.merged, cache) {
                count += 1;
                if let Some(lim) = opts.limit {
                    if count >= lim as u64 { break; }
                }
            }
        }
    } else {
        if let Some(candidates) = intersect_index(&index_keys, cache) {
            for id in &candidates {
                if let Some(rec) = cache.by_id.get(id.as_str()) {
                    if !opts.include_trash && rec.is_trash() { continue; }
                    if matches(filter, &rec.merged, cache) {
                        count += 1;
                        if let Some(lim) = opts.limit {
                            if count >= lim as u64 { break; }
                        }
                    }
                }
            }
        }
    }

    count
}

/// Extract single-tag Has terms from the top-level AND chain of a filter.
///
/// Returns the list of tag names that can be used to compute an initial
/// candidate set via the tag_index. An empty result means fall back to
/// full scan.
///
/// Rules:
///   Has(path) where path.len() == 1  → contribute the tag name
///   And(a, b)                        → contribute from both branches
///   Or / anything else               → return empty (cannot safely use index)
fn extract_index_keys(filter: &Filter) -> Vec<String> {
    let mut keys = Vec::new();
    collect_index_keys(filter, &mut keys);
    keys
}

fn collect_index_keys(filter: &Filter, out: &mut Vec<String>) {
    match filter {
        Filter::Has(path) if path.len() == 1 => {
            out.push(path.first().to_string());
        }
        Filter::And(a, b) => {
            collect_index_keys(a, out);
            collect_index_keys(b, out);
        }
        // Everything else (Or, Eq, Ne, comparisons, IsSpec, IsSymbol, Missing,
        // multi-segment Has) does not contribute index keys, but does not
        // invalidate keys already collected from sibling AND branches.
        // An Or at the root simply contributes nothing, leaving keys empty
        // and triggering a full scan — the safe conservative choice.
        _ => {}
    }
}

/// Intersect the tag_index sets for the given keys.
///
/// Returns None if any key has no entry in the index (empty intersection).
/// Returns Some(set) with the candidate record ids.
fn intersect_index<'a>(keys: &[String], cache: &'a RecordCache) -> Option<HashSet<String>> {
    // Start with the smallest set to minimize intersection work
    let mut sets: Vec<&HashSet<String>> = keys.iter()
        .filter_map(|k| cache.tag_index.get(k))
        .collect();

    if sets.len() < keys.len() {
        // At least one key has no index entry — intersection is empty
        return None;
    }

    // Sort by set size ascending for efficient intersection
    sets.sort_by_key(|s| s.len());

    let mut result: HashSet<String> = sets[0].clone();
    for s in &sets[1..] {
        result = result.intersection(s).cloned().collect();
        if result.is_empty() { return None; }
    }
    Some(result)
}

/// Get display string for a record Dict.
pub fn dict_dis(dict: &Dict) -> String {
    if let Some(Val::Str(s)) = dict.get("dis") {
        return s.clone();
    }
    if let Some(Val::Ref(r)) = dict.get("id") {
        return r.dis.clone().unwrap_or_else(|| r.id.clone());
    }
    String::new()
}
