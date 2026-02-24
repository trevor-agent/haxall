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
}

impl Default for QueryOpts {
    fn default() -> Self {
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
    for_each_candidate(filter, opts, cache, |dict| {
        results.push(dict.clone());
        opts.limit.map_or(true, |lim| results.len() < lim)
    });
    if opts.sort {
        results.sort_by(|a, b| dict_dis(a).cmp(&dict_dis(b)));
    }
    results
}

/// Count records matching a filter (no materialization).
pub fn read_count(
    filter: &Filter,
    opts:   &QueryOpts,
    cache:  &RecordCache,
) -> u64 {
    let mut count = 0u64;
    for_each_candidate(filter, opts, cache, |_| {
        count += 1;
        opts.limit.map_or(true, |lim| count < lim as u64)
    });
    count
}

/// Core candidate loop: calls `visit(dict)` for each record that passes the filter.
/// `visit` returns `true` to continue, `false` to stop early (limit support).
///
/// Uses the tag index to intersect candidate sets when the filter's leading AND
/// chain contains simple Has(tag) terms; falls back to a full scan otherwise.
fn for_each_candidate<F>(
    filter: &Filter,
    opts:   &QueryOpts,
    cache:  &RecordCache,
    mut visit: F,
) where F: FnMut(&Dict) -> bool {
    let index_keys = extract_index_keys(filter);

    if index_keys.is_empty() {
        for (_, rec) in &cache.by_id {
            if !opts.include_trash && rec.is_trash() { continue; }
            if !matches(filter, &rec.merged, cache) { continue; }
            if !visit(&rec.merged) { return; }
        }
    } else if let Some(candidates) = intersect_index(&index_keys, cache) {
        for id in &candidates {
            if let Some(rec) = cache.by_id.get(id.as_str()) {
                if !opts.include_trash && rec.is_trash() { continue; }
                if !matches(filter, &rec.merged, cache) { continue; }
                if !visit(&rec.merged) { return; }
            }
        }
    }
    // If any index key has no entries, intersect is empty — zero results.
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
        _ => {}
    }
}

/// Intersect the tag_index sets for the given keys.
///
/// Returns None if any key has no entry in the index (empty intersection).
/// Returns Some(set) with the candidate record ids.
fn intersect_index<'a>(keys: &[String], cache: &'a RecordCache) -> Option<HashSet<String>> {
    let mut sets: Vec<&HashSet<String>> = keys.iter()
        .filter_map(|k| cache.tag_index.get(k))
        .collect();

    if sets.len() < keys.len() {
        return None;
    }

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
