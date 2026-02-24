// benches/filter_bench.rs — filter parsing and evaluation benchmarks.
//
// Measures filter::parse() and query::read_all() in isolation against a pre-seeded
// in-memory RecordCache. No I/O, no IPC — pure Rust-internal cost.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_folio::{
    filter,
    query::{self, QueryOpts},
};

#[path = "bench_common.rs"]
mod common;

// ── Filter parsing ────────────────────────────────────────────────────────────

fn bench_filter_parse(c: &mut Criterion) {
    let cases = [
        ("simple",   "equip"),
        ("compound", "equip and point and his"),
        ("path_eq",  "equipRef->siteRef->geoCity == \"Chicago\""),
        ("num_cmp",  "temp > 72"),
        ("or_root",  "equip or device"),
        ("is_spec",  "ph::Equip"),
    ];

    let mut group = c.benchmark_group("filter/parse");
    for (name, expr) in &cases {
        group.bench_with_input(BenchmarkId::from_parameter(name), expr, |b, &expr| {
            b.iter(|| black_box(filter::parse(expr).unwrap()))
        });
    }
    group.finish();
}

// ── Filter evaluation (read_all) ─────────────────────────────────────────────

fn bench_filter_has_simple(c: &mut Criterion) {
    let filter = filter::parse("equip").unwrap();
    let opts   = QueryOpts::default();

    let mut group = c.benchmark_group("filter/has_simple");
    for n in [1_000usize, 10_000, 100_000] {
        let cache = common::make_cache(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
        });
    }
    group.finish();
}

fn bench_filter_has_compound(c: &mut Criterion) {
    let filter = filter::parse("equip and point and his").unwrap();
    let opts   = QueryOpts::default();

    let mut group = c.benchmark_group("filter/has_compound");
    for n in [1_000usize, 10_000, 100_000] {
        let cache = common::make_cache(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
        });
    }
    group.finish();
}

fn bench_filter_comparison_eq(c: &mut Criterion) {
    // Matches exactly one record (dis == "Equip-0").
    let filter = filter::parse("dis == \"Equip-0\"").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache(10_000);

    c.bench_function("filter/comparison_eq", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_filter_comparison_num(c: &mut Criterion) {
    // Matches ~1/3 of point records that have a `temp` tag, and among those,
    // about half exceed 72 (since temp ranges 20–59).
    let filter = filter::parse("temp > 30").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache(10_000);

    c.bench_function("filter/comparison_num", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_filter_path_traversal(c: &mut Criterion) {
    // Two-hop Ref deref: record.equipRef → equip.siteRef → site.geoCity == "Chicago"
    let filter = filter::parse("equipRef->siteRef->geoCity == \"Chicago\"").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache(10_000);

    c.bench_function("filter/path_traversal", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_filter_or_root(c: &mut Criterion) {
    // OR at the root forces a full scan (index cannot be used).
    let filter = filter::parse("site or equip").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache(10_000);

    c.bench_function("filter/or_root", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_filter_is_spec(c: &mut Criterion) {
    let filter = filter::parse("ph::Equip").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache_with_spec(10_000);

    c.bench_function("filter/is_spec", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_filter_no_match(c: &mut Criterion) {
    // Zero results — measures early-exit cost when index intersection is empty.
    let filter = filter::parse("ahu and chiller").unwrap();
    let opts   = QueryOpts::default();
    let cache  = common::make_cache(10_000);

    c.bench_function("filter/no_match", |b| {
        b.iter(|| black_box(query::read_all(&filter, &opts, &cache).len()))
    });
}

fn bench_read_count(c: &mut Criterion) {
    let filter = filter::parse("equip and his").unwrap();
    let opts   = QueryOpts::default();

    let mut group = c.benchmark_group("filter/read_count");
    for n in [10_000usize, 100_000] {
        let cache = common::make_cache(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(query::read_count(&filter, &opts, &cache)))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_filter_parse,
    bench_filter_has_simple,
    bench_filter_has_compound,
    bench_filter_comparison_eq,
    bench_filter_comparison_num,
    bench_filter_path_traversal,
    bench_filter_or_root,
    bench_filter_is_spec,
    bench_filter_no_match,
    bench_read_count,
);
criterion_main!(benches);
