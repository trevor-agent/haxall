# rustFolio Benchmark Results

> **Status:** Not yet run — scaffold only  
> **Methodology:** See `BENCHMARKS.md` for full description of each tier, warmup strategy, and GC tracking approach  
> **Platform:** Trevor's Mac mini (Apple Silicon, arm64)

---

## 0. Build Metrics (Tier 0)

> Run: `./bench_tier0.sh` from `rust/`

| Metric | Value |
|--------|-------|
| Release binary size | — |
| Direct dependencies | — |
| Total transitive deps | — |
| Clean release compile | — |
| Incremental compile | — |

---

## 1. Rust Microbenchmarks (Tier 1)

> Run: `cargo bench` from `rust/`  
> Full HTML reports in `rust/target/criterion/` (not committed)

### 1.1 Filter Evaluation

| Benchmark | Mean | Std Dev | Notes |
|-----------|------|---------|-------|
| `filter/has_simple` (1k) | — | — | |
| `filter/has_simple` (10k) | — | — | |
| `filter/has_simple` (100k) | — | — | |
| `filter/has_compound` (1k) | — | — | |
| `filter/has_compound` (10k) | — | — | |
| `filter/has_compound` (100k) | — | — | indexed vs. simple delta |
| `filter/comparison_eq` | — | — | single-result scan |
| `filter/comparison_num` | — | — | |
| `filter/path_traversal` | — | — | 2-hop Ref deref |
| `filter/or_root` | — | — | forced full scan |
| `filter/is_spec` | — | — | subtypes map lookup |
| `filter/no_match` | — | — | early exit |
| `filter/read_count` (10k) | — | — | |
| `filter/read_count` (100k) | — | — | |

### 1.2 Cache Operations

| Benchmark | Mean | Std Dev | Notes |
|-----------|------|---------|-------|
| `cache/read_by_id` hit (10k) | — | — | HashMap lookup |
| `cache/read_by_id` miss (10k) | — | — | |
| `cache/read_by_id` hit (100k) | — | — | |
| `cache/read_all_indexed` (1k) | — | — | |
| `cache/read_all_indexed` (10k) | — | — | |
| `cache/read_all_indexed` (100k) | — | — | |
| `cache/read_all_full_scan` (1k) | — | — | |
| `cache/read_all_full_scan` (10k) | — | — | |
| `cache/read_all_full_scan` (100k) | — | — | |
| `cache/load` (1k) | — | — | startup cost |
| `cache/load` (10k) | — | — | |
| `cache/load` (100k) | — | — | |
| `cache/apply_persistent_commit` | — | — | |
| `cache/apply_transient_commit` | — | — | curVal update |

### 1.3 Serialization

| Benchmark | Mean | Std Dev | Notes |
|-----------|------|---------|-------|
| `proto/serialize_small` | — | — | 5-tag dict |
| `proto/serialize_large` | — | — | 40-tag dict |
| `proto/roundtrip_small` | — | — | |
| `proto/roundtrip_large` | — | — | |
| `proto/serialize_grid_100` | — | — | 100-dict response |

### 1.4 Storage

| Benchmark | Mean | Std Dev | Notes |
|-----------|------|---------|-------|
| `storage/commit_single` | — | — | includes fsync |
| `storage/commit_batch/10` | — | — | |
| `storage/commit_batch/100` | — | — | |
| `storage/commit_batch/1000` | — | — | |
| `storage/load_all` (1k) | — | — | |
| `storage/load_all` (10k) | — | — | |
| `storage/load_all` (100k) | — | — | |
| `storage/his_write/100` | — | — | |
| `storage/his_write/1000` | — | — | |
| `storage/his_write/10000` | — | — | |
| `storage/his_read_full` (1k) | — | — | |
| `storage/his_read_full` (10k) | — | — | |
| `storage/his_read_full` (100k) | — | — | |
| `storage/his_read_span_10pct` | — | — | span = 10% of range |
| `storage/his_stat` | — | — | incremental stat |

---

## 2. Fantom Integration Benchmarks (Tier 2)

> Run: `fan fan/bench/BenchFolio.fan --backend both --recs 10000 --iters 1000 --warmup 200`  
> GC tracking: `FAN_JAVA_OPTS="-verbose:gc" fan fan/bench/BenchFolio.fan --backend hx 2>gc.log`

### 2.1 Read Benchmarks (10k records)

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| readById (ipc floor) | rust | — | — | — | — |
| readById (ipc floor) | hx | — | — | — | — |
| readAll equip | rust | — | — | — | — |
| readAll equip | hx | — | — | — | — |
| readAll equip+point+his | rust | — | — | — | — |
| readAll equip+point+his | hx | — | — | — | — |
| readAll no-match | rust | — | — | — | — |
| readAll no-match | hx | — | — | — | — |
| readCount equip | rust | — | — | — | — |
| readCount equip | hx | — | — | — | — |

### 2.2 Write Benchmarks (10k records pre-seeded)

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| commitAll add batch=1 | rust | — | — | — | — |
| commitAll add batch=1 | hx | — | — | — | — |
| commitAll add batch=10 | rust | — | — | — | — |
| commitAll add batch=10 | hx | — | — | — | — |
| commitAll add batch=100 | rust | — | — | — | — |
| commitAll add batch=100 | hx | — | — | — | — |

### 2.3 History Benchmarks (rustFolio only)

| Scenario | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|--------|--------|--------|
| hisWrite 100 items | — | — | — | — |
| hisWrite 1000 items | — | — | — | — |
| hisWrite 10000 items | — | — | — | — |
| hisRead full (10k items) | — | — | — | — |
| hisStat | — | — | — | — |

### 2.4 GC Metrics (hxFolio runs)

> Captured from `-verbose:gc` output; explains p99 tail latency

| Scenario | GC Pause Count | Total Pause (ms) | Max Pause (ms) |
|----------|----------------|------------------|----------------|
| readAll equip (1000 iters) | — | — | — |
| commitAll batch=100 (iters) | — | — | — |

---

## 3. Key Findings

> To be filled in after benchmark run

### IPC Floor

**`readById` delta (rustFolio vs. hxFolio): `—` µs per call**

This is the baseline overhead paid by every rustFolio operation. Operations that
exceed this delta by a significant margin are dominated by Rust computation cost.
Operations within 2× of this delta are IPC-bound.

### Batch Write Crossover

**Batch size at which rustFolio outperforms hxFolio: `—`**

Expected hypothesis: rustFolio wins at batch ≥ 100 (redb amortizes one WAL commit
across the entire batch; hxFolio writes one zinc file per record).

### Tail Latency (hxFolio)

**p99/p50 ratio for hxFolio readAll: `—`**

GC pause explanation: `—` pauses, `—` ms total during measurement window.

---

## 4. Run Log

| Date | Commit | Scenario | Notes |
|------|--------|----------|-------|
| — | — | — | First run |
