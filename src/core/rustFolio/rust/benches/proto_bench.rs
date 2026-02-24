// benches/proto_bench.rs — wire protocol serialization benchmarks.
//
// Measures write_dict / read_dict roundtrip cost in isolation.
// These numbers represent the per-message encoding overhead paid on every RPC call.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_folio::{types_de::read_dict, types_ser::write_dict};

#[path = "bench_common.rs"]
mod common;

fn bench_serialize_small(c: &mut Criterion) {
    let dict = common::make_small_dict();
    c.bench_function("proto/serialize_small", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(128);
            write_dict(&mut buf, black_box(&dict));
            black_box(buf.len())
        })
    });
}

fn bench_serialize_large(c: &mut Criterion) {
    let dict = common::make_large_dict();
    c.bench_function("proto/serialize_large", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(2048);
            write_dict(&mut buf, black_box(&dict));
            black_box(buf.len())
        })
    });
}

fn bench_roundtrip_small(c: &mut Criterion) {
    let dict = common::make_small_dict();
    let mut buf = Vec::new();
    write_dict(&mut buf, &dict);

    c.bench_function("proto/roundtrip_small", |b| {
        b.iter(|| {
            let mut pos = 0usize;
            black_box(read_dict(black_box(&buf), &mut pos).unwrap())
        })
    });
}

fn bench_roundtrip_large(c: &mut Criterion) {
    let dict = common::make_large_dict();
    let mut buf = Vec::new();
    write_dict(&mut buf, &dict);

    c.bench_function("proto/roundtrip_large", |b| {
        b.iter(|| {
            let mut pos = 0usize;
            black_box(read_dict(black_box(&buf), &mut pos).unwrap())
        })
    });
}

fn bench_serialize_grid(c: &mut Criterion) {
    // Simulate a readAll response: a Vec of 100 small dicts serialized sequentially.
    let dicts: Vec<_> = (0..100).map(|_| common::make_small_dict()).collect();
    c.bench_function("proto/serialize_grid_100", |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(100 * 128);
            for d in black_box(&dicts) {
                write_dict(&mut buf, d);
            }
            black_box(buf.len())
        })
    });
}

criterion_group!(
    benches,
    bench_serialize_small,
    bench_serialize_large,
    bench_roundtrip_small,
    bench_roundtrip_large,
    bench_serialize_grid,
);
criterion_main!(benches);
