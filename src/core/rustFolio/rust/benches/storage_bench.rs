// benches/storage_bench.rs — redb storage operation benchmarks.
//
// Measures Storage::commit_records, Storage::load_all_records, and history
// read/write directly against a real redb file (tempdir). These numbers reflect
// the raw persistence layer cost, excluding IPC and cache overhead.
//
// Note: commit benchmarks include a full fsync (default flush mode). This is
// intentional — it reflects real production behavior.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_folio::storage::Storage;

#[path = "bench_common.rs"]
mod common;

// ── Record commits ────────────────────────────────────────────────────────────

fn bench_commit_single(c: &mut Criterion) {
    let dir  = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bench.redb");
    let storage = Storage::open(&path).expect("Storage::open");
    let mut ver = 0u64;

    c.bench_function("storage/commit_single", |b| {
        b.iter(|| {
            ver += 1;
            let id   = format!("r{}", ver);
            let dict = common::make_bench_record(ver as usize);
            let changes = vec![(id, Some(dict))];
            black_box(storage.commit_records(black_box(&changes), ver - 1).unwrap());
        })
    });
}

fn bench_commit_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage/commit_batch");
    for batch_size in [10usize, 100, 1_000] {
        let dir     = tempfile::tempdir().expect("tempdir");
        let path    = dir.path().join(format!("bench_{}.redb", batch_size));
        let storage = Storage::open(&path).expect("Storage::open");
        let mut ver = 0u64;

        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &sz| {
                b.iter(|| {
                    ver += 1;
                    let base = (ver as usize) * sz;
                    let changes: Vec<_> = (0..sz)
                        .map(|i| {
                            let id   = format!("r{}", base + i);
                            let dict = common::make_bench_record(base + i);
                            (id, Some(dict))
                        })
                        .collect();
                    black_box(storage.commit_records(black_box(&changes), ver - 1).unwrap());
                })
            },
        );
    }
    group.finish();
}

// ── Bulk load ─────────────────────────────────────────────────────────────────

fn bench_load_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage/load_all");
    for n in [1_000usize, 10_000, 100_000] {
        // Pre-populate a database with n records, then measure load_all_records().
        let dir     = tempfile::tempdir().expect("tempdir");
        let path    = dir.path().join(format!("load_{}.redb", n));
        let storage = Storage::open(&path).expect("Storage::open");

        // Seed in batches of 500 to keep commit sizes reasonable.
        for chunk_start in (0..n).step_by(500) {
            let end = (chunk_start + 500).min(n);
            let changes: Vec<_> = (chunk_start..end)
                .map(|i| (format!("r{}", i), Some(common::make_bench_record(i))))
                .collect();
            storage.commit_records(&changes, chunk_start as u64).expect("commit");
        }

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(storage.load_all_records().unwrap().len()))
        });
    }
    group.finish();
}

// ── History write ─────────────────────────────────────────────────────────────

fn bench_his_write(c: &mut Criterion) {
    let dir  = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("his.redb");
    let storage = Storage::open(&path).expect("Storage::open");

    // Pre-seed a "point" record.
    storage.commit_records(&[("pt1".to_string(), Some(common::make_bench_record(1)))], 0)
        .expect("commit pt1");

    // Build val bytes (Number: f64 8 bytes big-endian, type tag = 0x04).
    fn make_num_bytes(v: f64) -> Vec<u8> {
        let mut b = vec![0x04u8]; // Number type tag
        b.extend_from_slice(&v.to_bits().to_be_bytes());
        b.extend_from_slice(&[0x00u8]); // no unit
        b
    }

    // Base ticks: 2024-01-01 00:00:00 UTC in nanoseconds (Fantom epoch: 2000-01-01)
    // Using i64 ticks relative to a convenient epoch; exact value doesn't matter for perf.
    let base_ticks: i64 = 757_382_400_000_000_000i64; // arbitrary recent timestamp in ns
    let minute_ns:  i64 = 60_000_000_000i64;

    let mut group = c.benchmark_group("storage/his_write");
    for n_items in [100usize, 1_000, 10_000] {
        let items: Vec<(i64, Vec<u8>)> = (0..n_items)
            .map(|i| (base_ticks + i as i64 * minute_ns, make_num_bytes(i as f64)))
            .collect();

        group.bench_with_input(BenchmarkId::from_parameter(n_items), &n_items, |b, _| {
            // Each iteration writes a fresh batch to a new point id so writes don't
            // accumulate state between iterations (avoiding unbounded DB growth in bench).
            let mut call_count = 0u64;
            b.iter(|| {
                call_count += 1;
                let id = format!("pt_w_{}", call_count);
                black_box(storage.his_write(&id, black_box(&items)).unwrap());
            })
        });
    }
    group.finish();
}

// ── History read ──────────────────────────────────────────────────────────────

fn bench_his_read_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage/his_read_full");

    for n_items in [1_000usize, 10_000, 100_000] {
        let dir     = tempfile::tempdir().expect("tempdir");
        let path    = dir.path().join(format!("his_r_{}.redb", n_items));
        let storage = Storage::open(&path).expect("Storage::open");

        storage.commit_records(&[("pt1".to_string(), Some(common::make_bench_record(1)))], 0)
            .expect("commit");

        let base_ticks: i64 = 757_382_400_000_000_000i64;
        let minute_ns:  i64 = 60_000_000_000i64;
        let items: Vec<(i64, Vec<u8>)> = (0..n_items)
            .map(|i| (base_ticks + i as i64 * minute_ns, vec![0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]))
            .collect();
        storage.his_write("pt1", &items).expect("seed history");

        group.bench_with_input(BenchmarkId::from_parameter(n_items), &n_items, |b, _| {
            b.iter(|| {
                black_box(storage.his_read("pt1", false, 0, 0).unwrap().len())
            })
        });
    }
    group.finish();
}

fn bench_his_read_span(c: &mut Criterion) {
    // 10k items seeded; span covers the middle 10%.
    let dir     = tempfile::tempdir().expect("tempdir");
    let path    = dir.path().join("his_span.redb");
    let storage = Storage::open(&path).expect("Storage::open");

    storage.commit_records(&[("pt1".to_string(), Some(common::make_bench_record(1)))], 0)
        .expect("commit");

    let n_items    = 10_000usize;
    let base_ticks: i64 = 757_382_400_000_000_000i64;
    let minute_ns:  i64 = 60_000_000_000i64;
    let items: Vec<(i64, Vec<u8>)> = (0..n_items)
        .map(|i| (base_ticks + i as i64 * minute_ns, vec![0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]))
        .collect();
    storage.his_write("pt1", &items).expect("seed");

    let span_start = base_ticks + (n_items as i64 / 2)     * minute_ns; // 50% in
    let span_end   = base_ticks + (n_items as i64 * 6 / 10) * minute_ns; // 60% in → 10% span

    c.bench_function("storage/his_read_span_10pct", |b| {
        b.iter(|| {
            black_box(storage.his_read("pt1", true, span_start, span_end).unwrap().len())
        })
    });
}

fn bench_his_stat(c: &mut Criterion) {
    let dir     = tempfile::tempdir().expect("tempdir");
    let path    = dir.path().join("his_stat.redb");
    let storage = Storage::open(&path).expect("Storage::open");

    storage.commit_records(&[("pt1".to_string(), Some(common::make_bench_record(1)))], 0)
        .expect("commit");

    let n_items    = 10_000usize;
    let base_ticks: i64 = 757_382_400_000_000_000i64;
    let minute_ns:  i64 = 60_000_000_000i64;
    let items: Vec<(i64, Vec<u8>)> = (0..n_items)
        .map(|i| (base_ticks + i as i64 * minute_ns, vec![0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]))
        .collect();
    storage.his_write("pt1", &items).expect("seed");

    c.bench_function("storage/his_stat", |b| {
        b.iter(|| black_box(storage.his_stat("pt1").unwrap().size))
    });
}

criterion_group!(
    benches,
    bench_commit_single,
    bench_commit_batch,
    bench_load_all,
    bench_his_write,
    bench_his_read_full,
    bench_his_read_span,
    bench_his_stat,
);
criterion_main!(benches);
