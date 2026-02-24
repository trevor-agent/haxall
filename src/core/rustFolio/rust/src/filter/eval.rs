// filter/eval.rs — Evaluate a Haystack filter against a Dict record.
//
// The evaluator needs access to the record cache for Ref dereferencing
// (path expressions like `ref->num == 10`).

use crate::types::{Dict, Val};
use crate::filter::ast::{Filter, FilterPath};
use crate::record_cache::RecordCache;

/// Returns true if the record matches the filter.
pub fn matches(filter: &Filter, rec: &Dict, cache: &RecordCache) -> bool {
    match_filter(filter, rec, cache)
}

fn match_filter(f: &Filter, rec: &Dict, cache: &RecordCache) -> bool {
    match f {
        Filter::Has(path)      => resolve_all(rec, path, cache).iter().any(|v| !matches!(v, Val::Null)),
        Filter::Missing(path)  => {
            let vals = resolve_all(rec, path, cache);
            vals.is_empty() || vals.iter().all(|v| matches!(v, Val::Null))
        }

        Filter::Eq(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o == 0),
        Filter::Ne(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o != 0),
        Filter::Lt(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o < 0),
        Filter::Le(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o <= 0),
        Filter::Gt(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o > 0),
        Filter::Ge(path, val)  => cmp_any(&resolve_all(rec, path, cache), val, |o| o >= 0),

        Filter::And(a, b) => match_filter(a, rec, cache) && match_filter(b, rec, cache),
        Filter::Or(a, b)  => match_filter(a, rec, cache) || match_filter(b, rec, cache),

        // isSpec: check if rec["spec"] is a subtype of the filter spec using the
        // spec_subtypes map populated via SPEC_UPDATE. Returns false if the map is
        // empty (ns not yet synced) or the spec is not in the hierarchy.
        Filter::IsSpec(name) => {
            let spec_id = match rec.get("spec") {
                Some(Val::Ref(r)) => r.id.as_str(),
                _ => return false,
            };
            match cache.spec_subtypes.get(name) {
                Some(set) => set.contains(spec_id),
                None      => false,
            }
        }

        // isSymbol: symbol-literal filters are not used by folio queries; always false.
        Filter::IsSymbol => false,
    }
}

/// Resolve all terminal values reachable via `path` from `rec`.
///
/// Returns a flat Vec of all leaf values, following list fan-out at every
/// segment. E.g. for `refx->ref->num` on a record where `refx` is a list
/// of Refs, each of which may have `ref` as a list of Refs, we return ALL
/// `num` values reachable through every combination. This lets `cmp_any`
/// check if ANY combination satisfies the filter predicate.
fn resolve_all(rec: &Dict, path: &FilterPath, cache: &RecordCache) -> Vec<Val> {
    let mut result = Vec::new();
    collect_vals(rec, &path.segments, cache, &mut result);
    result
}

fn collect_vals(rec: &Dict, segs: &[String], cache: &RecordCache, out: &mut Vec<Val>) {
    let val = match rec.get(&segs[0]) {
        Some(v) => v.clone(),
        None    => return,
    };

    if segs.len() == 1 {
        // Leaf segment: flatten list values directly into output
        match val {
            Val::List(items) => {
                for item in items {
                    if !matches!(item, Val::Null) {
                        out.push(item);
                    }
                }
            }
            other => out.push(other),
        }
        return;
    }

    // Intermediate segment: dereference Refs or fan out through List<Ref>
    match &val {
        Val::Ref(r) => {
            if let Some(target) = cache.get(&r.id) {
                collect_vals(&target.merged, &segs[1..], cache, out);
            }
        }
        Val::List(items) => {
            for item in items {
                if let Val::Ref(r) = item {
                    if let Some(target) = cache.get(&r.id) {
                        collect_vals(&target.merged, &segs[1..], cache, out);
                    }
                }
            }
        }
        _ => {} // non-ref intermediate → dead end
    }
}

/// Returns true if ANY value in `vals` satisfies `pred` when compared to `filter_val`.
/// For Ne specifically, returns true if ANY value differs (any-ne semantics).
fn cmp_any(vals: &[Val], filter_val: &Val, pred: impl Fn(i32) -> bool) -> bool {
    if vals.is_empty() { return false; }
    vals.iter().any(|v| {
        match compare_vals(v, filter_val) {
            None    => false,
            Some(o) => pred(o),
        }
    })
}

/// Compare two values. Returns None if types are incompatible.
/// Returns Some(negative/zero/positive) for ordering.
fn compare_vals(a: &Val, b: &Val) -> Option<i32> {
    match (a, b) {
        (Val::Bool(x),   Val::Bool(y))   => Some(bool_ord(*x, *y)),
        (Val::Number(x, _), Val::Number(y, _)) => Some(float_ord(*x, *y)),
        (Val::Str(x),    Val::Str(y))    => Some(str_ord(x, y)),
        (Val::Ref(x),    Val::Ref(y))    => Some(str_ord(&x.id, &y.id)),
        (Val::Marker,    Val::Marker)    => Some(0),
        (Val::NA,        Val::NA)        => Some(0),
        (Val::Null,      Val::Null)      => Some(0),

        (Val::Date(x),     Val::Date(y))     => Some(ord_to_i32(x.cmp(y))),
        (Val::Time(x),     Val::Time(y))     => Some(ord_to_i32(x.cmp(y))),
        (Val::DateTime(x), Val::DateTime(y)) => Some(ord_to_i32(x.cmp(y))),

        // Ref vs Str: compare ref.id with string (filter `id == "p:foo:r:bar"` style)
        (Val::Ref(r), Val::Str(s)) => Some(str_ord(&r.id, s)),
        (Val::Str(s), Val::Ref(r)) => Some(str_ord(s, &r.id)),

        // For List on the left: the filter path handler in resolve_path deals with
        // list-of-Refs specially. If we end up here with a list, treat as incompatible.
        _ => None,
    }
}

fn bool_ord(a: bool, b: bool) -> i32 {
    match (a, b) {
        (false, true)  => -1,
        (true, false)  =>  1,
        _              =>  0,
    }
}

fn float_ord(a: f64, b: f64) -> i32 {
    if a < b { -1 } else if a > b { 1 } else { 0 }
}

fn str_ord(a: &str, b: &str) -> i32 {
    ord_to_i32(a.cmp(b))
}

fn ord_to_i32(o: std::cmp::Ordering) -> i32 {
    match o {
        std::cmp::Ordering::Less    => -1,
        std::cmp::Ordering::Equal   =>  0,
        std::cmp::Ordering::Greater =>  1,
    }
}
