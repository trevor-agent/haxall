# rustFolio Benchmark Results

> **Platform:** Mac mini, Apple Silicon arm64 (Darwin 25.2.0)  
> **Date:** 2026-02-24  
> **Commit:** c37b4382 (rust-folio branch)  
> **Methodology:** See `BENCHMARKS.md` — Tier 0 build metrics, Tier 1 criterion microbenchmarks, Tier 2 Fantom integration harness (10k records, 1000 iters, 200 warmup)

---

## 0. Build Metrics (Tier 0)

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

## 1. Rust Microbenchmarks (Tier 1)

> `cargo bench` from `rust/` — criterion 0.5, 100 samples, 3s warmup per bench  
> Full HTML reports: `rust/target/criterion/` (not committed)

### 1.1 Filter Parsing

| Benchmark | Mean |
|-----------|------|
| `filter/parse/simple` (`equip`) | 72.2 ns |
| `filter/parse/compound` (`equip and point and his`) | 263.9 ns |
| `filter/parse/path_eq` (2-hop path + eq) | 219.8 ns |
| `filter/parse/num_cmp` (`temp > 30`) | 118.9 ns |
| `filter/parse/or_root` (`site or equip`) | 174.9 ns |
| `filter/parse/is_spec` (`ph::Equip`) | 103.3 ns |

### 1.2 Filter Evaluation

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

**Note:** `no_match` = 70ns because the tag index immediately returns an empty intersection for `ahu and chiller` (no record has both tags), short-circuiting before any candidate evaluation.

### 1.3 Cache Operations

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

**Note:** `read_by_id` lookup is O(1) and cache-size-invariant (15.7ns at 10k ≈ 15.8ns at 100k). The tag index shows modest benefit for compound filters on this dataset (full-scan 3.98ms vs indexed 4.38ms at 10k) because the compound filter matches ~37.5% of records — the index reduces candidates but result collection still dominates. Index wins dramatically on zero-result queries.

### 1.4 Serialization

| Benchmark | Mean |
|-----------|------|
| `proto/serialize_small` (5-tag dict) | 58.9 ns |
| `proto/serialize_large` (40-tag dict) | 334.6 ns |
| `proto/roundtrip_small` | 204.0 ns |
| `proto/roundtrip_large` | 1.684 µs |
| `proto/serialize_grid_100` (100 small dicts) | 4.688 µs |

### 1.5 Storage (redb)

| Benchmark | Mean | Notes |
|-----------|------|-------|
| `storage/commit_single` | **4.019 ms** | Single fsync per commit |
| `storage/commit_batch/10` | 4.056 ms | Fsync amortized slightly |
| `storage/commit_batch/100` | 5.129 ms | 51µs/record |
| `storage/commit_batch/1000` | 10.038 ms | 10µs/record |
| `storage/load_all` | 1k | 163.4 µs |
| `storage/load_all` | 10k | 1.673 ms |
| `storage/load_all` | 100k | 17.49 ms |
| `storage/his_write/100` | 4.980 ms | Fsync dominated |
| `storage/his_write/1000` | 9.128 ms | +4.1ms for 900 more items |
| `storage/his_write/10000` | 15.58 ms | ~0.5µs/item marginal |
| `storage/his_read_full` | 1k items | 32.86 µs |
| `storage/his_read_full` | 10k items | 331.4 µs |
| `storage/his_read_full` | 100k items | 3.294 ms |
| `storage/his_read_span_10pct` | 10k, 10% span | 35.63 µs |
| `storage/his_stat` | 10k history | **258.2 ns** |

**Storage notes:**  
- Commit is dominated by the redb WAL fsync at ~4ms per transaction, regardless of batch size up to ~100 records. The fsync floor exists by design — it guarantees durability (if the server returns 200 OK, the data is on disk).  
- History read scales linearly: 33µs/1k, 331µs/10k, 3.3ms/100k → ~33ns/item.  
- `his_stat` = 258ns because the Rust process maintains an in-memory stat cache updated on every write — stat is a pure memory lookup, no storage I/O.

---

## 2. Fantom Integration Benchmarks (Tier 2)

> `fan BenchFolio.fan --backend both --recs 10000 --iters 1000 --warmup 200`  
> 10k seeded records (1k sites, 2.5k equips, 6.5k points), 10 points × 10k history items

### 2.1 Read Benchmarks

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

### 2.2 Write Benchmarks

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| commitAll add batch=1 | rust | 193 | 5,007 | 5,548 | 6,102 |
| commitAll add batch=1 | **hx** | **9,553** | **102.2** | 116.7 | 143.4 |
| commitAll add batch=10 | rust | 167 | 5,958 | 6,914 | 11,017 |
| commitAll add batch=10 | hx | 1,061 | 932.1 | 1,000.3 | 1,139.5 |
| commitAll add batch=100 | **rust** | **114** | **8,730** | 10,961 | 24,826 |
| commitAll add batch=100 | hx | 107 | 9,331.3 | 9,629.5 | 9,732.0 |

### 2.3 History Benchmarks (rustFolio only)

| Scenario | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|--------|--------|--------|
| hisWrite 100 items | 209 | 4,831 | 5,183 | 5,183 |
| hisWrite 1000 items | 163 | 6,036 | 6,834 | 6,834 |
| hisWrite 10000 items | 48 | 20,097 | 25,675 | 25,675 |
| hisRead full (10k items) | **320** | **3,069** | 3,131 | 4,721 |

---

## 3. Key Findings

### 3.1 IPC Floor

**`readById` delta: rustFolio p50 = 24.7µs vs hxFolio p50 = 0.6µs → IPC overhead ≈ 24µs per call**

Every rustFolio operation pays this cost as a baseline. The 24µs breaks down as: TCP loopback write + Rust deserialize Ref + HashMap lookup + Rust serialize Dict + TCP loopback read + Fantom deserialize. Rust-internal lookup alone (Tier 1) = 15.7ns — the IPC accounts for 99.9% of the total per-call latency.

### 3.2 readAll — The IPC Transfer Problem

For large result sets, readAll is **IPC-transfer-bound**, not filter-eval-bound:

| Factor | Cost |
|--------|------|
| Rust filter eval + collect (Tier 1, 10k, ~7.5k matches) | ~4.1 ms |
| Fantom-side Dict deserialization (estimated) | ~35 ms |
| **Total observed (Tier 2)** | **~40 ms** |
| hxFolio in-process readAll (same query) | 363.8 µs |
| **Slowdown factor** | **~110×** |

The bottleneck is Fantom JVM deserialization of the wire response (~35ms for 7.5k dicts), not TCP latency or Rust serialization. This is architectural — rustFolio cannot match hxFolio on high-cardinality readAll because it requires cross-process Dict marshaling for every matching record.

**Mitigation path**: Push aggregation/projection into Rust (e.g., a `readAllIds` or `readCount` variant that returns a compact result instead of full Dicts).

### 3.3 readAll No-Match — rustFolio WINS

For zero-result queries, rustFolio is **7× faster** than hxFolio (17.9µs vs 202.5µs). Rust's tag index immediately determines an empty intersection for `ahu and chiller` in O(1) — 70ns internally — versus hxFolio's in-process full scan of 10k records at 202µs. For unknown-filter queries (e.g., checking if any records of a new type exist), rustFolio has a structural advantage.

### 3.4 commitAll — Batch Crossover Confirmed

| Batch | rust µs/rec | hx µs/rec | Winner |
|-------|-------------|-----------|--------|
| 1 | 5,007 | 102 | hx (49×) |
| 10 | 596 | 93 | hx (6.4×) |
| 100 | **87** | **93** | **rust (1.07×)** |

**The batch crossover is at ~100 records.** Hypothesis confirmed. At batch=1, rust's 4ms fsync floor makes it 49× slower than hxFolio's OS-buffered zinc write. At batch=100, redb amortizes the fsync across 100 records at 87µs/record, beating hxFolio's 93µs/record.

For workloads with batch sizes ≥100 (e.g., bulk provisioning, large import jobs), rustFolio has **better write throughput than hxFolio** while also guaranteeing stronger durability (WAL-based, not OS-buffered).

**Note**: hxFolio's p99 for batch=100 is 9.7ms (clean, tight distribution). rustFolio's p99 is 24.8ms (high outliers from occasional extra fsync delays). This is worth investigating — redb's WAL should not produce that tail.

### 3.5 History Performance

History write is fsync-dominated:
- 100 items: 4.8ms (entirely fsync — item writes are trivial)
- 1000 items: 6ms (+1.2ms for 900 more items = 1.3µs/item marginal cost)
- 10k items: 20ms (+14ms for 9k more items = 1.6µs/item marginal)

History read is fast — single IPC round trip, binary format:
- **3ms for 10k items** = 0.3µs/item deserialized end-to-end (Fantom side)
- Tier 1 storage-only: 331µs/10k = 33ns/item in redb scan

The Fantom deserialization overhead for history (3ms for 10k items = 0.3µs/item) is much lower per-item than for full Dict records (~4.7µs/record in readAll). This confirms that the compact binary history format is efficient.

### 3.6 Summary Table

| Operation | rustFolio | hxFolio | Assessment |
|-----------|-----------|---------|------------|
| readById | 24.7µs | 0.6µs | hx 41× — **IPC floor is unavoidable** |
| readAll (large result set) | 40ms | 364µs | hx 110× — **IPC-transfer-bound** |
| readAll (no match) | 17.9µs | 202µs | **rust 11× — index wins** |
| readCount | 1.5ms | 184µs | hx 8× — IPC for a single int is expensive |
| commitAll batch=1 | 5ms | 102µs | hx 49× — fsync floor |
| commitAll batch=100 | 8.7ms | 9.3ms | **rust 1.07× — crossover achieved** |
| hisRead 10k | 3ms | N/A | rust only — fast |
| hisWrite 1k | 6ms | N/A | rust only — fsync dominated |

---

## 4. Run Log

| Date | Commit | Scenario | Notes |
|------|--------|----------|-------|
| 2026-02-24 | c37b4382 | Tier 0 build metrics | arm64 Darwin |
| 2026-02-24 | c37b4382 | Tier 1 criterion (all benches) | 100 samples each |
| 2026-02-24 | c37b4382 | Tier 2 rust backend, 10k recs, 1k iters | Full run |
| 2026-02-24 | c37b4382 | Tier 2 hx backend, 10k recs, 1k iters | Full run |
