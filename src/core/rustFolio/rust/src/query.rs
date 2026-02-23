// query.rs — ReadAll implementation with opts support.
//
// Opts dict keys (matches Fantom folio opts):
//   trash: Marker  — include trash records (default: exclude)
//   limit: Number  — max records to return
//   sort:  Marker  — sort results by dis (display string)

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

    for (_id, rec) in &cache.by_id {
        // Trash filtering
        if !opts.include_trash && rec.is_trash() {
            continue;
        }

        // Filter evaluation
        if !matches(filter, &rec.merged, cache) {
            continue;
        }

        results.push(rec.merged.clone());

        // Apply limit early to avoid collecting too many
        if let Some(lim) = opts.limit {
            if results.len() >= lim {
                break;
            }
        }
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
    for (_id, rec) in &cache.by_id {
        if !opts.include_trash && rec.is_trash() { continue; }
        if matches(filter, &rec.merged, cache) {
            count += 1;
            if let Some(lim) = opts.limit {
                if count >= lim as u64 { break; }
            }
        }
    }
    count
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
