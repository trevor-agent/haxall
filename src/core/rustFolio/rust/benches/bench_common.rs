// benches/bench_common.rs — shared test-data builders for all benchmark files.
#![allow(dead_code)]
//
// Included via `#[path = "bench_common.rs"] mod common;` at the top of each bench file.

use chrono::Utc;
use rust_folio::{
    record_cache::RecordCache,
    types::*,
};

/// Build a RecordCache pre-seeded with `n` records.
///
/// Distribution (approximate):
///   n/10  site  records: {id, dis, site, geoCity}
///   n/4   equip records: {id, dis, equip, ahu, siteRef}
///   rest  point records: {id, dis, point, equip, his, siteRef, equipRef, kind, unit}
///                        one-third also have a numeric `temp` tag
///
/// Every id is a plain string (no prefix) so filter tests work without a config.
pub fn make_cache(n: usize) -> RecordCache {
    let n_sites  = (n / 10).max(1);
    let n_equips = (n / 4).max(1);
    let n_points = n.saturating_sub(n_sites + n_equips);
    let now      = Utc::now().with_timezone(&chrono_tz::UTC);

    let mut records: Vec<(String, Dict)> = Vec::with_capacity(n);

    for i in 0..n_sites {
        let id   = format!("s{}", i);
        let city = if i % 2 == 0 { "Chicago" } else { "NewYork" };
        let mut d = Dict::new();
        d.set("id",      Val::Ref(HRef::new(id.clone())));
        d.set("dis",     Val::Str(format!("Site-{}", i)));
        d.set("geoCity", Val::Str(city.to_string()));
        d.set("mod",     Val::DateTime(now.clone()));
        d.set("site",    Val::Marker);
        records.push((id, d));
    }

    for i in 0..n_equips {
        let id      = format!("e{}", i);
        let site_id = format!("s{}", i % n_sites);
        let mut d = Dict::new();
        d.set("ahu",     Val::Marker);
        d.set("dis",     Val::Str(format!("Equip-{}", i)));
        d.set("equip",   Val::Marker);
        d.set("id",      Val::Ref(HRef::new(id.clone())));
        d.set("mod",     Val::DateTime(now.clone()));
        d.set("siteRef", Val::Ref(HRef::new(site_id)));
        records.push((id, d));
    }

    for i in 0..n_points {
        let id       = format!("p{}", i);
        let equip_id = format!("e{}", i % n_equips);
        let site_id  = format!("s{}", (i % n_equips) % n_sites);
        let mut d = Dict::new();
        d.set("dis",      Val::Str(format!("Point-{}", i)));
        d.set("equip",    Val::Marker);
        d.set("equipRef", Val::Ref(HRef::new(equip_id)));
        d.set("his",      Val::Marker);
        d.set("id",       Val::Ref(HRef::new(id.clone())));
        d.set("kind",     Val::Str("Number".to_string()));
        d.set("mod",      Val::DateTime(now.clone()));
        d.set("point",    Val::Marker);
        d.set("siteRef",  Val::Ref(HRef::new(site_id)));
        d.set("unit",     Val::Str("kW".to_string()));
        if i % 3 == 0 {
            d.set("temp", Val::Number(20.0 + (i % 40) as f64, Some("°C".to_string())));
        }
        records.push((id, d));
    }

    let mut cache = RecordCache::new(0);
    cache.load(records);
    cache
}

/// Build a cache with isSpec support seeded (one parent + N subtypes).
pub fn make_cache_with_spec(n: usize) -> RecordCache {
    let mut cache = make_cache(n);
    // Seed spec_subtypes: "ph::Equip" has itself + "ph::Ahu" as subtypes.
    let mut equip_set = std::collections::HashSet::new();
    equip_set.insert("ph::Equip".to_string());
    equip_set.insert("ph::Ahu".to_string());
    equip_set.insert("ph::Chiller".to_string());
    cache.spec_subtypes.insert("ph::Equip".to_string(), equip_set);
    // Tag a few records with spec = ph::Ahu
    for (_, rec) in cache.by_id.iter_mut() {
        if rec.merged.get("ahu").is_some() {
            rec.persistent.set("spec", Val::Ref(HRef::new("ph::Ahu")));
            rec.merged.set("spec",     Val::Ref(HRef::new("ph::Ahu")));
        }
    }
    cache
}

/// Minimal 5-tag Dict for serialization benchmarks.
pub fn make_small_dict() -> Dict {
    let now = Utc::now().with_timezone(&chrono_tz::UTC);
    let mut d = Dict::new();
    d.set("ahu",   Val::Marker);
    d.set("dis",   Val::Str("AHU-1".to_string()));
    d.set("equip", Val::Marker);
    d.set("id",    Val::Ref(HRef::new("e1")));
    d.set("mod",   Val::DateTime(now));
    d
}

/// Dense 40-tag Dict for serialization benchmarks.
pub fn make_large_dict() -> Dict {
    let now = Utc::now().with_timezone(&chrono_tz::UTC);
    let mut d = Dict::new();
    d.set("ahu",     Val::Marker);
    d.set("dis",     Val::Str("AHU-1".to_string()));
    d.set("equip",   Val::Marker);
    d.set("id",      Val::Ref(HRef::new("e1")));
    d.set("mod",     Val::DateTime(now));
    d.set("siteRef", Val::Ref(HRef { id: "s1".to_string(), dis: Some("Site-1".to_string()) }));
    // Pad to ~40 tags
    for i in 0..34usize {
        d.set(format!("xTag{:02}", i), Val::Str(format!("value-{}", i)));
    }
    d
}

/// Build a commit-ready Dict for add benchmarks (no `id` tag — folio assigns it).
pub fn make_bench_record(i: usize) -> Dict {
    let mut d = Dict::new();
    d.set("bench", Val::Marker);
    d.set("dis",   Val::Str(format!("Bench-{}", i)));
    d.set("idx",   Val::Number(i as f64, None));
    d
}
