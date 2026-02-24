# rustFolio Benchmark Plan

> **Status:** Active — implementation in progress  
> **Goal:** Comprehensive performance characterization of the rustFolio backend against itself at scale and against hxFolio as baseline

---

## 1. Goals

1. **Characterize** — absolute throughput and latency for every major operation
2. **Compare** — rustFolio vs. hxFolio on operations where both are supported
3. **Scale** — identify where performance degrades and at what record/batch counts
4. **Isolate** — separate Rust-internal cost from IPC cost so we know where to optimize if needed

The most architecturally important number in this entire plan is the **IPC floor**:
the `readById` latency delta between rustFolio and hxFolio. Every rustFolio operation
pays this cost. Knowing it determines whether future optimization effort belongs in
Rust code or the IPC layer.

---

## 2. Architecture

Four independent benchmark tiers. Each measures something different; all are necessary.

```
┌──────────────────────────────────────────────────────────┐
│  Tier 0: Build metrics                                   │
│  What:   binary size, dep count, compile time            │
│  Tool:   cargo, wc, shell commands                       │
│  Value:  dep-hygiene baseline; reviewer will ask         │
└──────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────┐
│  Tier 1: Rust microbenchmarks (criterion)                │
│  What:   filter eval, cache ops, serialization, storage  │
│  Tool:   cargo bench  →  target/criterion/               │
│  Value:  isolates Rust-internal cost, no IPC noise       │
│  Warmup: handled automatically by criterion              │
└──────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────┐
│  Tier 2: Fantom integration benchmarks                   │
│  What:   full folio API: readAll, commitAll, his*, open  │
│  Tool:   BenchFolio.fan harness, runs both backends      │
│  Value:  real-world path, directly comparable to hxFolio │
│  Warmup: --warmup N flag (untimed iterations before      │
│          measurement; required for JVM JIT stability)    │
│  GC:     -verbose:gc for hxFolio runs to capture pause   │
│          count + total pause time (explains p99 tail)    │
└──────────────────────────────────────────────────────────┘
```

Tier 2 is the headline. Tier 1 tells us *why*. Tier 0 answers the reviewer's first
question before they ask it.

---

## 3. Tier 0 — Build Metrics

Captured once before running any performance benchmarks. Snapshot into
`BENCHMARKS_RESULTS.md` section 0.

| Metric | Command | Notes |
|--------|---------|-------|
| Release binary size | `ls -lh target/release/rust-folio` | Stripped size |
| Direct dependencies | `cargo tree --depth 1 \| wc -l` | Cargo.toml deps only |
| Total transitive deps | `cargo tree \| grep -c "^[a-z]"` | Full dep tree |
| Clean release compile | `time cargo build --release` (after `cargo clean`) | Cold build |
| Incremental compile | `time cargo build --release` (after touching one .rs file) | Hot build |

No pass/fail criteria — these are baselines. Record and move on.

---

## 4. Tier 1 — Rust Microbenchmarks

**Location:** `rust/benches/`  
**Tooling:** `criterion = { version = "0.5", features = ["html_reports"] }` (dev-dep only)  
**Run:** `cargo bench` — generates HTML reports in `target/criterion/`

Criterion handles warmup, outlier detection, and statistical confidence intervals
automatically. No manual warmup needed for Tier 1.

### 4.1 Filter Evaluation (`benches/filter_bench.rs`)

Measures `filter::matches()` + `query::read_all()` in isolation against an in-memory
RecordCache. Inputs are pre-seeded; no I/O during timing.

| Benchmark | Filter | Cache Size | Notes |
|-----------|--------|------------|-------|
| `filter/has_simple` | `equip` | 10k | Single-tag; exercises tag index |
| `filter/has_compound` | `equip and point and his` | 10k | Index intersection of 3 sets |
| `filter/has_compound_100k` | `equip and point and his` | 100k | Scale comparison |
| `filter/comparison_eq` | `dis == "Pump-1"` | 10k | Eq on string tag |
| `filter/comparison_num` | `temp > 72` | 10k | Numeric comparison |
| `filter/path_traversal` | `equip->siteRef->geoCity == "Chicago"` | 10k | 2-hop Ref deref |
| `filter/or_root` | `equip or device` | 10k | OR root: forces full scan |
| `filter/is_spec` | `ph::Equip` | 10k | IsSpec: subtypes map lookup |
| `filter/no_match` | `ahu and chiller` | 10k | Zero result — early exit |

Each benchmark runs the full `read_all()` path (index lookup → candidate filter →
collect). Separately benchmark `filter::parse()` for representative filter strings —
parse cost should be negligible vs. eval but worth confirming.

### 4.2 Cache Operations (`benches/cache_bench.rs`)

| Benchmark | Operation | Scale |
|-----------|-----------|-------|
| `cache/read_by_id` | HashMap lookup | 10k, 100k |
| `cache/read_all_full_scan` | No-index filter | 1k, 10k, 100k |
| `cache/read_all_indexed` | 3-tag index intersection | 1k, 10k, 100k |
| `cache/apply_persistent_commit` | Single record update + index diff | 10k recs |
| `cache/apply_transient_commit` | Transient overlay update | 10k recs |
| `cache/load` | Bulk load all records from Vec | 1k, 10k, 100k |

The `read_all_indexed` vs. `read_all_full_scan` delta quantifies the tag index payoff.

### 4.3 Serialization (`benches/proto_bench.rs`)

| Benchmark | Operation | Notes |
|-----------|-----------|-------|
| `proto/serialize_dict_small` | 5-tag record Dict | Typical equip record |
| `proto/serialize_dict_large` | 40-tag record Dict | Dense record |
| `proto/deserialize_dict_small` | Round-trip parse | Same data |
| `proto/deserialize_dict_large` | Round-trip parse | Dense record |
| `proto/serialize_grid` | 100-row grid | readAll response payload |

These isolate wire protocol overhead from everything else.

### 4.4 Storage (`benches/storage_bench.rs`)

Direct redb operations, no cache or IPC.

| Benchmark | Operation | Scale |
|-----------|-----------|-------|
| `storage/commit_single` | Write 1 record to redb | — |
| `storage/commit_batch_10` | Write 10 records in one txn | — |
| `storage/commit_batch_100` | Write 100 records in one txn | — |
| `storage/commit_batch_1000` | Write 1000 records in one txn | — |
| `storage/load_all` | `load_all_records()` full scan | 1k, 10k, 100k |
| `storage/his_write_100` | Write 100 history items | — |
| `storage/his_write_1000` | Write 1000 history items | — |
| `storage/his_write_10000` | Write 10k history items | — |
| `storage/his_read_full` | Read all items for a point | 1k, 10k, 100k items |
| `storage/his_read_span` | Span query (~10% of range) | 10k items |

The `commit_batch_*` series characterizes how batch size affects redb write throughput.
The critical question: does redb's WAL amortize transaction overhead across batch sizes?

---

## 5. Tier 2 — Fantom Integration Benchmarks

**Location:** `fan/bench/BenchFolio.fan`  
**Tooling:** Standalone Fantom script; timing via `Duration.now()`  
**Run:** `fan BenchFolio.fan [--backend rust|hx|both] [--recs N] [--iters N] [--warmup N]`  
**Output:** Markdown table + raw CSV

### 5.1 JVM Warmup

JVM benchmarks without warmup include JIT compilation cost that does not exist at
steady state. This makes early iterations artificially slow and causes hxFolio
numbers to look worse than they actually are, producing unfair comparisons.

The `--warmup N` flag runs N **untimed** iterations of each scenario before
measurement begins. Default: `--warmup 200`. The warmup iterations are excluded
from all statistics. For scenarios where N < 200 total iterations are being measured
(e.g., the 100k-record startup test), warmup is capped at `min(warmup, iters/2)`.

The warmup block runs the same operations as the measurement block. For hxFolio,
200 warmup iterations of `readAll("equip")` are sufficient to drive JIT compilation
of the hot paths. For rustFolio, the JVM-side is trivially thin (serialize args →
TCP write → read response → deserialize), so warmup primarily exercises the Fantom
serializer — but it runs on both backends regardless for consistency.

### 5.2 GC Pause Tracking (hxFolio runs)

When running against the hxFolio backend, the JVM is launched with `-verbose:gc`
to capture garbage collection pause information. The harness:

1. Pipes JVM stderr to a temp file during the benchmark run.
2. After measurement completes, parses the GC log to extract:
   - **Pause count** — number of GC pauses during the measurement window
   - **Total pause time** — sum of all pause durations (ms)
   - **Max single pause** — longest individual pause (ms)
3. Reports these alongside the latency percentiles in the results table.

This directly explains p99 tail latency for hxFolio. When p99 is 10× p50, the GC
log will show exactly how many pauses occurred and whether they account for the
tail. Without this, the tail looks like noise — with it, we know whether to tune the
JVM or accept the behavior as structural to a garbage-collected runtime.

For rustFolio runs, no GC flag is applied (there is no GC on the Rust side; the JVM
overhead on the Fantom client is minimal and shared with hxFolio).

### 5.3 Startup

| Scenario | Metric | Backends |
|----------|--------|----------|
| `startup/open_empty` | Time from `Folio.open()` to first query | rust, hx |
| `startup/open_1k` | Open + cache load, 1k existing records | rust, hx |
| `startup/open_10k` | Open + cache load, 10k records | rust, hx |
| `startup/open_100k` | Open + cache load, 100k records | rust, hx |

For rustFolio this includes: spawn subprocess → redb scan → cache load → IPC
handshake. For hxFolio: zinc file scan → cache load.

### 5.4 Read Operations

All iterations use a pre-seeded database (setup excluded from timing). Warmup
iterations precede each scenario.

| Scenario | Filter / Op | Records | Iterations | Backends |
|----------|-------------|---------|------------|----------|
| `read/by_id` | `readById(id)` | 10k | 10k | rust, hx |
| `read/all_simple` | `readAll("equip")` | 1k, 10k, 100k | 1k | rust, hx |
| `read/all_compound` | `readAll("equip and point and his")` | 10k | 1k | rust, hx |
| `read/all_path` | `readAll("equip->siteRef->geoCity")` | 10k | 1k | rust, hx |
| `read/all_no_match` | `readAll("ahu and chiller")` | 10k | 1k | rust, hx |
| `read/count` | `readCount("equip")` | 10k | 10k | rust, hx |

Report: **ops/sec** and **p50/p95/p99 latency** across iterations.

`read/by_id` is the IPC floor measurement. Both backends are cache-hits. The latency
delta is the per-call IPC overhead. Every other rustFolio result is this number plus
the cost of the operation itself.

### 5.5 Write Operations

| Scenario | Op | Batch size | Iterations | Backends |
|----------|----|------------|------------|----------|
| `commit/add_single` | `commitAll([Diff.makeAdd(...)])` | 1 | 5k | rust, hx |
| `commit/add_batch_10` | `commitAll([...])` | 10 | 1k | rust, hx |
| `commit/add_batch_100` | `commitAll([...])` | 100 | 500 | rust, hx |
| `commit/add_batch_1000` | `commitAll([...])` | 1000 | 100 | rust, hx |
| `commit/update_single` | Update 1 existing record | 1 | 5k | rust, hx |
| `commit/update_batch_100` | Update 100 existing records | 100 | 500 | rust, hx |
| `commit/remove_single` | Remove 1 record | 1 | 5k | rust, hx |
| `commit/transient_single` | Transient overlay commit | 1 | 10k | rust only |
| `commit/transient_batch_100` | Transient overlay commit | 100 | 1k | rust only |

Transient commits are rustFolio-only (hxFolio doesn't implement `supportsTransient()`).
Batch write performance is a primary expected advantage of redb over zinc file I/O.

### 5.6 History Operations

hxFolio does not implement the history API. These are rustFolio-only.

| Scenario | Op | Items | Points | Iterations |
|----------|----|-------|--------|------------|
| `his/write_100` | `hisWrite` | 100 | 1 | 1k |
| `his/write_1000` | `hisWrite` | 1000 | 1 | 500 |
| `his/write_10000` | `hisWrite` | 10000 | 1 | 100 |
| `his/write_multipoint_100` | Rapid sequential writes, 100 points | 100/pt | 100 pts | 10 |
| `his/read_full_1k` | `hisRead` no span | 1k items | 1 | 1k |
| `his/read_full_10k` | `hisRead` no span | 10k items | 1 | 500 |
| `his/read_full_100k` | `hisRead` no span | 100k items | 1 | 100 |
| `his/read_span_10pct` | Span (~10% of range) | 10k items | 1 | 500 |
| `his/stat` | `hisStat` | 10k items | 1 | 10k |

Note: `his/write_multipoint_100` measures rapid sequential submissions to the folio
actor for 100 different points — not parallel execution. The folio actor processes
one request at a time; this scenario measures actor mailbox throughput under a
multi-point write workload.

### 5.7 Mixed Workload (Realistic)

Simulates a running Haxall instance: interleaved reads and writes submitted
sequentially to the folio actor, which queues and processes them one at a time.
This is not a concurrency benchmark — it measures actor mailbox throughput and how
operation mix affects overall throughput.

| Scenario | Read % | Write % | His % | Duration | Backends |
|----------|--------|---------|-------|----------|----------|
| `mixed/read_heavy` | 90 | 5 | 5 | 30s | rust, hx |
| `mixed/write_heavy` | 50 | 40 | 10 | 30s | rust, hx |
| `mixed/his_heavy` | 40 | 10 | 50 | 30s | rust only |

Each scenario submits operations in the specified ratio, sequentially, for the
given duration. Reports: total ops, ops/sec, operation mix breakdown, error count.

---

## 6. Scalability Matrix

| Metric | 1k recs | 5k recs | 10k recs | 50k recs | 100k recs |
|--------|---------|---------|----------|----------|-----------|
| Startup time | — | — | — | — | — |
| `readAll("equip")` ops/sec | — | — | — | — | — |
| `readAll("equip and point")` ops/sec | — | — | — | — | — |
| `commitAll` single-record latency p50 | — | — | — | — | — |
| Memory RSS (rust) | — | — | — | — | — |
| Memory RSS (hx) | — | — | — | — | — |

Memory tracked via `ProcessBuilder.out` piped to `ps -o rss` after the seeding
phase, before benchmark timing begins.

---

## 7. What to Measure

| Metric | Unit | Notes |
|--------|------|-------|
| Throughput | ops/sec | Primary metric for bulk operations |
| p50 latency | µs or ms | Typical-case latency |
| p95 latency | µs or ms | Tail latency |
| p99 latency | µs or ms | Worst-case outliers |
| GC pause count | count | hxFolio only; from -verbose:gc log |
| GC total pause | ms | hxFolio only; sum of all pauses |
| GC max pause | ms | hxFolio only; longest single pause |

When hxFolio shows a high p99/p50 ratio, the GC metrics will explain whether it is
attributable to garbage collection or to something else (disk I/O, zinc parse, etc.).

---

## 8. Expected Outcomes & Hypotheses

| Area | Hypothesis | Confidence |
|------|-----------|------------|
| `readById` IPC floor | rustFolio ~50–200µs slower per call (TCP loopback + ser/deser) | High |
| `readAll` simple, large dataset | rustFolio faster (Rust filter eval vs. Fantom JVM) | Medium |
| `readAll` compound, indexed | rustFolio comparable or faster (tag index intersection in Rust) | Medium |
| `commitAll` single record | rustFolio slower (redb txn + IPC overhead vs. zinc file write) | Medium |
| `commitAll` batch ≥100 | rustFolio faster (redb batches in one txn; zinc writes N files) | High |
| History write | Linear with batch size; redb should beat any file-based alternative | High |
| Startup, 100k records | rustFolio slightly slower (redb scan + subprocess spawn vs. zinc scan) | Low |
| p99 tail latency hxFolio | GC pauses account for most p99 outliers | Medium |
| p99 tail latency rustFolio | Low — no GC; occasional OS scheduling noise only | Medium |

---

## 9. Implementation Plan

### Phase 1 — Tier 0 (15 min)

Shell script `bench_tier0.sh` that captures binary size, dep counts, compile times.
Run once, paste output into `BENCHMARKS_RESULTS.md` §0.

### Phase 2 — Tier 1 Criterion Setup (1 session)

```
rust/benches/
├── filter_bench.rs
├── cache_bench.rs
├── proto_bench.rs
└── storage_bench.rs
```

Add to `Cargo.toml`:
```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
tempfile = "3"

[[bench]]
name = "filter_bench"
harness = false

[[bench]]
name = "cache_bench"
harness = false

[[bench]]
name = "proto_bench"
harness = false

[[bench]]
name = "storage_bench"
harness = false
```

### Phase 3 — Tier 2 Fantom Harness (1 session)

```
fan/bench/
└── BenchFolio.fan
```

Key implementation details:
- `--warmup N` (default 200): untimed iterations before measurement
- `-verbose:gc` on hxFolio JVM; parse GC log after each scenario
- `Duration.now()` around each operation; collect into `Duration[]`; compute
  p50/p95/p99 from sorted array
- Output: markdown table printed to stdout + raw CSV to `bench_results.csv`

### Phase 4 — Run & Record (1 session)

1. `./bench_tier0.sh > tier0.txt`
2. `cargo bench 2>&1 | tee tier1.txt`
3. `fan BenchFolio.fan --backend both --recs 10000 --warmup 200 --iters 1000`
4. `fan BenchFolio.fan --backend both --recs 1000,5000,10000,50000,100000 --warmup 100 --iters 500`
5. Document all results in `BENCHMARKS_RESULTS.md`

---

## 10. Benchmark Data Generation

Records seeded with realistic tag structures:

```fantom
// Typical "equip" record — 8 tags
{id, dis, equip, siteRef, equipRef, ahu, hvac, mod}

// Typical "point" record — 12 tags
{id, dis, point, equip, his, siteRef, equipRef, kind, unit, sensor, cur, writable, mod}
```

History seeded with uniform 1-minute interval ticks, `Number` values.
Seeding pass is excluded from benchmark timing.

---

## 11. Deliverables

| Artifact | Description |
|----------|-------------|
| `rust/benches/*.rs` | Criterion benchmark files |
| `fan/bench/BenchFolio.fan` | Fantom integration harness |
| `BENCHMARKS_RESULTS.md` | Final results with tables and analysis |
| `target/criterion/` | Criterion HTML reports (local, not committed) |

---

## Notes & Constraints

- Benchmarks run on Trevor's Mac mini (arm64, Apple Silicon) — note CPU/memory spec in results
- All Tier 2 benchmarks run sequentially through the folio actor — threading is not relevant to this design
- hxFolio does not support history API — history benchmarks are rustFolio-only
- GC pause tracking (`-verbose:gc`) requires the JVM to be launched by the harness, not via `fan` wrapper — harness must construct the JVM command directly
- `cargo bench` requires a release-profile Rust binary — same binary used in production
- If results change substantially after any code change, re-run the affected tier and note the commit hash in `BENCHMARKS_RESULTS.md`
