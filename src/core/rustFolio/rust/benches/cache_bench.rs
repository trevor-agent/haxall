// benches/cache_bench.rs — RecordCache operation benchmarks.
//
// Isolates cache-level costs: HashMap lookup, bulk load, commit application,
// and the indexed vs. full-scan delta.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_folio::{
    filter,
    query::{self, QueryOpts},
    record_cache::RecordCache,
    types::{Dict, Val},
};

#[path = "bench_common.rs"]
mod common;

// ── readById: cache lookup ────────────────────────────────────────────────────

fn bench_cache_read_by_id(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/read_by_id");
    for n in [10_000usize, 100_000] {
        let cache    = common::make_cache(n);
        let hit_id   = format!("e{}", n / 8);
        let miss_id  = "e999999".to_string();
        group.bench_with_input(BenchmarkId::new("hit", n), &n, |b, _| {
            b.iter(|| black_box(cache.get(&hit_id)))
        });
        group.bench_with_input(BenchmarkId::new("miss", n), &n, |b, _| {
            b.iter(|| black_box(cache.get(&miss_id)))
        });
    }
    group.finish();
}

// ── readAll: indexed vs. full-scan ───────────────────────────────────────────

fn bench_cache_read_all_indexed(c: &mut Criterion) {
    let filter = filter::parse("equip and point and his").unwrap();
    let opts   = QueryOpts::default();

    let mut group = c.benchmark_group("cache/read_all_indexed");
    for n in [1_000usize, 10_000, 100_000] {
        let cache = common::make_cache(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
        });
    }
    group.finish();
}

fn bench_cache_read_all_full_scan(c: &mut Criterion) {
    let filter = filter::parse("site or equip").unwrap();
    let opts   = QueryOpts::default();

    let mut group = c.benchmark_group("cache/read_all_full_scan");
    for n in [1_000usize, 10_000, 100_000] {
        let cache = common::make_cache(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
        });
    }
    group.finish();
}

// ── Cache bulk load ───────────────────────────────────────────────────────────

fn bench_cache_load(c: &mut Criterion) {
    // Measures RecordCache::load() — the startup cost of scanning all records
    // from storage into the in-memory cache. build_records() runs outside timing.
    let mut group = c.benchmark_group("cache/load");
    for n in [1_000usize, 10_000, 100_000] {
        let records: Vec<(String, Dict)> = {
            // Build synthetic records without going through make_cache,
            // since we need owned (String, Dict) pairs.
            let tmp = common::make_cache(n);
            // Re-create from the base data so we can call load() fresh each iteration.
            // We pre-build once and re-use across iterations (load is the subject, not build).
            let mut v: Vec<(String, Dict)> = Vec::with_capacity(n);
            for (id, rec) in tmp.by_id {
                v.push((id, rec.persistent));
            }
            v
        };

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut cache = RecordCache::new(0);
                // We can't clone Dict, so re-build from scratch each iteration.
                // This measures only the load() cost on pre-owned data.
                // The Vec is re-built each outer iter call; the bench timing includes
                // the Vec build — but that cost is linear in n and consistent.
                let mut local: Vec<(String, Dict)> = Vec::with_capacity(records.len());
                for (id, _) in &records {
                    local.push((id.clone(), common::make_bench_record(0)));
                }
                cache.load(local);
                black_box(cache.by_id.len())
            })
        });
    }
    group.finish();
}

// ── Commit application ────────────────────────────────────────────────────────

fn bench_cache_apply_persistent_commit(c: &mut Criterion) {
    // Pre-seed a 10k cache; apply repeated updates to the same record.
    // Measures: dict replacement, tag_index diff, merged recomputation.
    let mut cache    = common::make_cache(10_000);
    let target_id    = "e1".to_string();
    let mut val: f64 = 0.0;

    c.bench_function("cache/apply_persistent_commit", |b| {
        b.iter(|| {
            val += 1.0;
            // Build a fresh Dict each iteration (can't clone)
            let mut updated = Dict::new();
            updated.set("ahu",   Val::Marker);
            updated.set("dis",   Val::Str(format!("Updated-{}", val)));
            updated.set("equip", Val::Marker);
            updated.set("id",    Val::Ref(rust_folio::types::HRef::new("e1")));
            cache.apply_persistent_commit(&target_id, Some(updated));
            black_box(cache.cur_ver())
        })
    });
}

fn bench_cache_apply_transient_commit(c: &mut Criterion) {
    // Apply rapid transient (curVal) updates — the most common real-world operation.
    // Each iteration adds/updates a single curVal tag; the tag key doesn't change
    // after the first iteration so index maintenance cost drops to near zero.
    let mut cache  = common::make_cache(10_000);
    let target_id  = "p1".to_string();
    let mut val: f64 = 0.0;

    c.bench_function("cache/apply_transient_commit", |b| {
        b.iter(|| {
            val += 1.0;
            let mut changes = Dict::new();
            changes.set("curVal", Val::Number(val, Some("kW".to_string())));
            cache.apply_transient_commit(&target_id, &changes);
            black_box(cache.cur_ver())
        })
    });
}

criterion_group!(
    benches,
    bench_cache_read_by_id,
    bench_cache_read_all_indexed,
    bench_cache_read_all_full_scan,
    bench_cache_load,
    bench_cache_apply_persistent_commit,
    bench_cache_apply_transient_commit,
);
criterion_main!(benches);
