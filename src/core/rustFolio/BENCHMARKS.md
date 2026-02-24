# rustFolio Benchmarks

> **Platform:** Mac mini, Apple Silicon arm64 (Darwin 25.2.0)  
> **Date:** 2026-02-24  
> **Commit:** c37b4382 (rust-folio branch)  
> **Gate:** 10,234 verifies, 0 failures, 0 warnings

Comprehensive performance characterization of the rustFolio backend. Tier 0
captures build metrics, Tier 1 isolates Rust-internal cost via criterion
microbenchmarks, and Tier 2 measures the full Fantom integration path (including
IPC) against hxFolio as baseline.

**The most architecturally important number:** the `readById` IPC floor (§3.1).
Every rustFolio operation pays this cost. Knowing it determines whether future
optimization effort belongs in Rust code or the IPC layer.

---

## 1. Build Metrics (Tier 0)

| Metric | Value |
|--------|-------|
| Release binary size (unstripped) | 3.6 MB |
| Release binary size (stripped) | 3.2 MB |
| Direct prod dependencies | 6 (chrono, chrono-tz, redb, thiserror, tracing, tracing-subscriber) |
| Direct dev dependencies | 2 (criterion, tempfile) |
| Total transitive (prod only) | 30 unique crates |
| Total transitive (all incl. dev/bench) | 73 unique crates |
| Clean release compile (real) | 6.2s |
| Clean release compile (user, parallel) | 36.8s |

---

## 2. Rust Microbenchmarks (Tier 1)

> `cargo bench` from `rust/` — criterion 0.5, 100 samples, 3s warmup per bench  
> Full HTML reports: `rust/target/criterion/` (not committed)

Tier 1 isolates Rust-internal cost: filter eval, cache ops, serialization, and
storage. No IPC, no Fantom — pure Rust performance.

### 2.1 Filter Parsing

| Benchmark | Mean |
|-----------|------|
| `filter/parse/simple` (`equip`) | 72.2 ns |
| `filter/parse/compound` (`equip and point and his`) | 263.9 ns |
| `filter/parse/path_eq` (2-hop path + eq) | 219.8 ns |
| `filter/parse/num_cmp` (`temp > 30`) | 118.9 ns |
| `filter/parse/or_root` (`site or equip`) | 174.9 ns |
| `filter/parse/is_spec` (`ph::Equip`) | 103.3 ns |

### 2.2 Filter Evaluation

| Benchmark | Cache | Mean |
|-----------|-------|------|
| `filter/has_simple` | 1k | 377.6 µs |
| `filter/has_simple` | 10k | 4.11 ms |
| `filter/has_simple` | 100k | 80.7 ms |
| `filter/has_compound` | 1k | 422.1 µs |
| `filter/has_compound` | 10k | 4.45 ms |
| `filter/has_compound` | 100k | 81.5 ms |
| `filter/comparison_eq` (`dis == "Equip-0"`) | 10k | 1.014 ms |
| `filter/comparison_num` (`temp > 30`) | 10k | 1.218 ms |
| `filter/path_traversal` (2-hop Ref deref) | 10k | 3.258 ms |
| `filter/or_root` (`site or equip` — full scan) | 10k | 4.022 ms |
| `filter/is_spec` (`ph::Equip`) | 10k | 1.096 ms |
| `filter/no_match` (`ahu and chiller` — early exit) | 10k | **70.3 ns** |
| `filter/read_count` | 10k | 1.485 ms |
| `filter/read_count` | 100k | 48.17 ms |

`no_match` = 70ns because the tag index immediately returns an empty intersection
for `ahu and chiller` (no record has both tags), short-circuiting before any
candidate evaluation.

### 2.3 Cache Operations

| Benchmark | Cache | Mean |
|-----------|-------|------|
| `cache/read_by_id` (hit) | 10k | 15.7 ns |
| `cache/read_by_id` (miss) | 10k | 7.1 ns |
| `cache/read_by_id` (hit) | 100k | 15.8 ns |
| `cache/read_by_id` (miss) | 100k | 7.8 ns |
| `cache/read_all_indexed` (`equip and point and his`) | 1k | 408 µs |
| `cache/read_all_indexed` | 10k | 4.38 ms |
| `cache/read_all_indexed` | 100k | 83.6 ms |
| `cache/read_all_full_scan` (`site or equip`) | 1k | 373 µs |
| `cache/read_all_full_scan` | 10k | 3.98 ms |
| `cache/read_all_full_scan` | 100k | 72.3 ms |
| `cache/load` | 1k | 466 µs |
| `cache/load` | 10k | 4.60 ms |
| `cache/load` | 100k | 84.8 ms |
| `cache/apply_persistent_commit` | 10k | 559 ns |
| `cache/apply_transient_commit` | 10k | 1.28 µs |

`read_by_id` is O(1) and cache-size-invariant (15.7ns at 10k ≈ 15.8ns at 100k).
The tag index shows modest benefit for compound filters on this dataset because
the compound filter matches ~37.5% of records — index wins dramatically on
zero-result queries.

### 2.4 Serialization

| Benchmark | Mean |
|-----------|------|
| `proto/serialize_small` (5-tag dict) | 58.9 ns |
| `proto/serialize_large` (40-tag dict) | 334.6 ns |
| `proto/roundtrip_small` | 204.0 ns |
| `proto/roundtrip_large` | 1.684 µs |
| `proto/serialize_grid_100` (100 small dicts) | 4.688 µs |

### 2.5 Storage (redb)

| Benchmark | Mean | Notes |
|-----------|------|-------|
| `storage/commit_single` | **4.019 ms** | Single fsync per commit |
| `storage/commit_batch/10` | 4.056 ms | Fsync amortized slightly |
| `storage/commit_batch/100` | 5.129 ms | 51µs/record |
| `storage/commit_batch/1000` | 10.038 ms | 10µs/record |
| `storage/load_all` (1k) | 163.4 µs | |
| `storage/load_all` (10k) | 1.673 ms | |
| `storage/load_all` (100k) | 17.49 ms | |
| `storage/his_write/100` | 4.980 ms | Fsync dominated |
| `storage/his_write/1000` | 9.128 ms | +4.1ms for 900 more items |
| `storage/his_write/10000` | 15.58 ms | ~0.5µs/item marginal |
| `storage/his_read_full` (1k items) | 32.86 µs | |
| `storage/his_read_full` (10k items) | 331.4 µs | |
| `storage/his_read_full` (100k items) | 3.294 ms | |
| `storage/his_read_span_10pct` (10k, 10% span) | 35.63 µs | |
| `storage/his_stat` (10k history) | **258.2 ns** | |

Commit is dominated by the redb WAL fsync at ~4ms per transaction, regardless of
batch size up to ~100 records. This is by design — it guarantees durability (if the
server returns OK, the data is on disk). History read scales linearly: ~33ns/item.
`his_stat` = 258ns (pure memory lookup, no storage I/O).

---

## 3. Fantom Integration Benchmarks (Tier 2)

> `fan BenchFolio.fan --backend both --recs 10000 --iters 1000 --warmup 200`  
> 10k seeded records (1k sites, 2.5k equips, 6.5k points), 10 points × 10k history items

Tier 2 measures the real-world path: Fantom → TCP → Rust → redb → TCP → Fantom.
Both backends run the same operations on the same data for direct comparison.

**Warmup:** 200 untimed iterations before measurement (required for JVM JIT
stability on hxFolio runs; applied to both backends for consistency).

### 3.1 Read Benchmarks

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| readById (**IPC floor**) | rust | 27,681 | **24.7** | 59.0 | 69.3 |
| readById | hx | 1,623,660 | **0.6** | 0.8 | 1.0 |
| readAll equip (~7.5k matches) | rust | 25 | 39,935 | 41,941 | 42,281 |
| readAll equip | hx | 2,697 | 363.8 | 399.1 | 424.1 |
| readAll equip+point+his (~3.75k matches) | rust | 30 | 33,225 | 35,282 | 35,706 |
| readAll equip+point+his | hx | 2,136 | 446.8 | 567.3 | 576.4 |
| readAll no-match (0 results) | **rust** | **36,660** | **17.9** | 62.8 | 68.9 |
| readAll no-match | hx | 5,001 | 202.5 | 224.0 | 232.1 |
| readCount equip | rust | 675 | 1,483.9 | 1,530.5 | 1,572.1 |
| readCount equip | hx | 5,232 | 183.6 | 217.4 | 232.3 |

### 3.2 Write Benchmarks

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| commitAll add batch=1 | rust | 193 | 5,007 | 5,548 | 6,102 |
| commitAll add batch=1 | **hx** | **9,553** | **102.2** | 116.7 | 143.4 |
| commitAll add batch=10 | rust | 167 | 5,958 | 6,914 | 11,017 |
| commitAll add batch=10 | hx | 1,061 | 932.1 | 1,000.3 | 1,139.5 |
| commitAll add batch=100 | **rust** | **114** | **8,730** | 10,961 | 24,826 |
| commitAll add batch=100 | hx | 107 | 9,331.3 | 9,629.5 | 9,732.0 |

### 3.3 History Benchmarks (rustFolio only)

| Scenario | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|--------|--------|--------|
| hisWrite 100 items | 209 | 4,831 | 5,183 | 5,183 |
| hisWrite 1000 items | 163 | 6,036 | 6,834 | 6,834 |
| hisWrite 10000 items | 48 | 20,097 | 25,675 | 25,675 |
| hisRead full (10k items) | **320** | **3,069** | 3,131 | 4,721 |

---

## 4. Key Findings

### 4.1 IPC Floor

**`readById` delta: rustFolio p50 = 24.7µs vs hxFolio p50 = 0.6µs → IPC overhead ≈ 24µs per call**

Every rustFolio operation pays this cost. The 24µs breaks down as: TCP loopback
write + Rust deserialize Ref + HashMap lookup + Rust serialize Dict + TCP loopback
read + Fantom deserialize. Rust-internal lookup alone (Tier 1) = 15.7ns — IPC
accounts for 99.9% of per-call latency.

This overhead is the cost of process isolation. hxFolio returns Java object
references (zero-copy, in-process). rustFolio must serialize every result across
a process boundary.

### 4.2 readAll — IPC Transfer Bound

For large result sets, readAll is **IPC-transfer-bound**, not filter-eval-bound:

| Factor | Cost |
|--------|------|
| Rust filter eval + collect (Tier 1, 10k, ~7.5k matches) | ~4.1 ms |
| Fantom-side Dict deserialization (estimated) | ~35 ms |
| **Total observed (Tier 2)** | **~40 ms** |
| hxFolio in-process readAll (same query) | 363.8 µs |

The bottleneck is Fantom JVM deserialization of the wire response (~35ms for 7.5k
dicts), not TCP latency or Rust serialization. This is architectural — rustFolio
cannot match hxFolio on high-cardinality readAll because it requires cross-process
Dict marshaling for every matching record.

### 4.3 readAll No-Match — rustFolio Wins

For zero-result queries, rustFolio is **11× faster** (17.9µs vs 202.5µs). Rust's
tag index determines an empty intersection in O(1) — 70ns internally — versus
hxFolio's in-process full scan of 10k records at 202µs.

### 4.4 Batch Write Crossover

| Batch | rust µs/rec | hx µs/rec | Winner |
|-------|-------------|-----------|--------|
| 1 | 5,007 | 102 | hx (49×) |
| 10 | 596 | 93 | hx (6.4×) |
| 100 | **87** | **93** | **rust (1.07×)** |

**Crossover at ~100 records.** At batch=1, rust's 4ms fsync floor makes it 49×
slower than hxFolio's OS-buffered zinc write. At batch=100, redb amortizes the
fsync across 100 records at 87µs/record, beating hxFolio's 93µs/record — while
also guaranteeing stronger durability (WAL-based, not OS-buffered).

### 4.5 History Performance

History write is fsync-dominated: 4.8ms for 100 items, 6ms for 1000 (+1.3µs/item
marginal). History read is fast: 3ms for 10k items end-to-end = 0.3µs/item.
The compact binary format is efficient — per-item deserialization cost (0.3µs) is
much lower than full Dict deserialization (~4.7µs/record in readAll).

### 4.6 Summary

| Operation | rustFolio | hxFolio | Assessment |
|-----------|-----------|---------|------------|
| readById | 24.7µs | 0.6µs | hx 41× — IPC floor, unavoidable |
| readAll (large result) | 40ms | 364µs | hx 110× — IPC-transfer-bound |
| readAll (no match) | **17.9µs** | 202µs | **rust 11×** — tag index |
| readCount | 1.5ms | 184µs | hx 8× — IPC for a single int |
| commitAll batch=1 | 5ms | 102µs | hx 49× — fsync floor |
| commitAll batch=100 | **8.7ms** | 9.3ms | **rust 1.07×** — crossover |
| hisRead 10k | 3ms | N/A | rust only — fast |
| hisWrite 1k | 6ms | N/A | rust only — fsync dominated |

**rustFolio trades single-process read speed for process isolation, durable storage,
crash recovery, and a native history API.** The IPC overhead (~24µs per call) is
the cost of those features.

---

## 5. Methodology

### 5.1 Tier 1 (Criterion)

Standard criterion 0.5 configuration: 100 samples per benchmark, 3-second warmup,
outlier detection enabled. Benchmarks use pre-seeded in-memory `RecordCache`
instances with realistic tag distributions (10% sites, 25% equips, 65% points with
Ref chains for path traversal tests).

### 5.2 Tier 2 (Fantom Harness)

`BenchFolio.fan` with `--warmup 200 --iters 1000 --recs 10000`. JVM warmup is
required for fair hxFolio comparisons — without it, JIT compilation noise makes
the first 50–100 iterations artificially slow. Both backends receive the same
warmup for consistency.

Timing via `Duration.now()` around each operation; percentiles computed from sorted
`Duration[]` array.

### 5.3 Data Generation

Records seeded with realistic Haystack tag structures:
- **Site** (8 tags): `{id, dis, site, geoCity, mod, ...}`
- **Equip** (8 tags): `{id, dis, equip, ahu, siteRef, mod, ...}`
- **Point** (12 tags): `{id, dis, point, equip, his, siteRef, equipRef, kind, unit, sensor, mod, ...}`
- **History**: 10k items per point, uniform 1-minute intervals, Number values

### 5.4 Not Yet Run

- **Scalability matrix** (1k → 100k records) — planned, not yet executed
- **Mixed workload** (30-second interleaved read/write/his simulations) — planned
- **GC pause tracking** (`-verbose:gc` for hxFolio tail latency attribution) — planned

---

## 6. Run Log

| Date | Commit | Scenario | Notes |
|------|--------|----------|-------|
| 2026-02-24 | c37b4382 | Tier 0 build metrics | arm64 Darwin |
| 2026-02-24 | c37b4382 | Tier 1 criterion (all benches) | 100 samples each |
| 2026-02-24 | c37b4382 | Tier 2 rust backend, 10k recs, 1k iters | Full run |
| 2026-02-24 | c37b4382 | Tier 2 hx backend, 10k recs, 1k iters | Full run |
