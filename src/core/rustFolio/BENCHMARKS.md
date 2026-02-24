# rustFolio Benchmark Plan

> **Status:** Draft — pending review before implementation  
> **Goal:** Comprehensive performance characterization of the rustFolio backend against itself at scale and against hxFolio as baseline

---

## 1. Goals

1. **Characterize** — absolute throughput and latency for every major operation
2. **Compare** — rustFolio vs. hxFolio on operations where both are supported
3. **Scale** — identify where performance degrades and at what record/batch counts
4. **Isolate** — separate Rust-internal cost from IPC cost so we know where to optimize if needed

---

## 2. Architecture

Two independent benchmark tiers. They measure different things and are both necessary.

```
┌──────────────────────────────────────────────────────────┐
│  Tier 1: Rust microbenchmarks (criterion)                │
│  What:   filter eval, cache ops, serialization, storage  │
│  Tool:   cargo bench  →  target/criterion/               │
│  Value:  isolates Rust-internal cost, no IPC noise       │
└──────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────┐
│  Tier 2: Fantom integration benchmarks                   │
│  What:   full folio API: readAll, commitAll, his*, open  │
│  Tool:   BenchFolio.fan harness, runs both backends      │
│  Value:  real-world path, directly comparable to hxFolio │
└──────────────────────────────────────────────────────────┘
```

Tier 2 is the headline. Tier 1 tells us *why*.

---

## 3. Tier 1 — Rust Microbenchmarks

**Location:** `rust/benches/`  
**Tooling:** `criterion = { version = "0.5", features = ["html_reports"] }` (dev-dep only)  
**Run:** `cargo bench` — generates HTML reports in `target/criterion/`

### 3.1 Filter Evaluation (`benches/filter_bench.rs`)

Measures `filter::matches()` + `query::read_all()` in isolation against an in-memory RecordCache.
Inputs are pre-seeded; no I/O during timing.

| Benchmark | Filter | Cache Size | Notes |
|-----------|--------|------------|-------|
| `filter/has_simple` | `equip` | 10k | Single-tag; exercises tag index |
| `filter/has_compound` | `equip and point and his` | 10k | Index intersection of 3 sets |
| `filter/has_compound` | `equip and point and his` | 100k | Scale comparison |
| `filter/comparison_eq` | `dis == "Pump-1"` | 10k | Eq on string tag |
| `filter/comparison_num` | `temp > 72` | 10k | Numeric comparison |
| `filter/path_traversal` | `equip->siteRef->geoCity == "Chicago"` | 10k | 2-hop Ref deref |
| `filter/or_root` | `equip or device` | 10k | OR root: forces full scan |
| `filter/is_spec` | `ph::Equip` | 10k | IsSpec: subtypes map lookup |
| `filter/no_match` | `ahu and chiller` | 10k | Zero result — early exit |

Each benchmark runs the full `read_all()` path (index lookup → candidate filter → collect).
Separately benchmark `filter::parse()` for a representative set of filter strings — parse cost
should be negligible vs. eval but worth confirming.

### 3.2 Cache Operations (`benches/cache_bench.rs`)

| Benchmark | Operation | Scale |
|-----------|-----------|-------|
| `cache/read_by_id` | HashMap lookup | 10k, 100k |
| `cache/read_all_full_scan` | No-index filter | 1k, 10k, 100k |
| `cache/read_all_indexed` | 3-tag index intersection | 1k, 10k, 100k |
| `cache/apply_persistent_commit` | Single record update + index diff | 10k recs |
| `cache/apply_transient_commit` | Transient overlay update | 10k recs |
| `cache/load` | Bulk load all records from Vec | 1k, 10k, 100k |

The `read_all_indexed` vs. `read_all_full_scan` delta quantifies the tag index payoff.

### 3.3 Serialization (`benches/proto_bench.rs`)

| Benchmark | Operation | Notes |
|-----------|-----------|-------|
| `proto/serialize_dict_small` | 5-tag record Dict | Typical equip record |
| `proto/serialize_dict_large` | 40-tag record Dict | Dense record |
| `proto/deserialize_dict_small` | Round-trip parse | Same data |
| `proto/deserialize_dict_large` | Round-trip parse | Dense record |
| `proto/serialize_grid` | 100-row grid | readAll response payload |

These isolate wire protocol overhead from everything else.

### 3.4 Storage (`benches/storage_bench.rs`)

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

## 4. Tier 2 — Fantom Integration Benchmarks

**Location:** `fan/bench/BenchFolio.fan`  
**Tooling:** Standalone Fantom script; timing via `Duration.now()`  
**Run:** `fan BenchFolio.fan [--backend rust|hx|both] [--recs N] [--iters N]`  
**Output:** Markdown table + raw CSV for further analysis

### 4.1 Startup

| Scenario | Metric | Backends |
|----------|--------|----------|
| `startup/open_empty` | Time from `Folio.open()` to first query | rust, hx |
| `startup/open_1k` | Open + cache load, 1k existing records | rust, hx |
| `startup/open_10k` | Open + cache load, 10k records | rust, hx |
| `startup/open_100k` | Open + cache load, 100k records | rust, hx |

For rustFolio this includes: spawn subprocess → redb scan → cache load → IPC handshake.  
For hxFolio: zinc file scan → cache load.

### 4.2 Read Operations

All iterations use a pre-seeded database (setup excluded from timing).

| Scenario | Filter / Op | Records | Iterations | Backends |
|----------|-------------|---------|------------|----------|
| `read/by_id` | `readById(id)` | 10k | 10k | rust, hx |
| `read/all_simple` | `readAll("equip")` | 1k, 10k, 100k | 1k | rust, hx |
| `read/all_compound` | `readAll("equip and point and his")` | 10k | 1k | rust, hx |
| `read/all_path` | `readAll("equip->siteRef->geoCity")` | 10k | 1k | rust, hx |
| `read/all_no_match` | `readAll("ahu and chiller")` | 10k | 1k | rust, hx |
| `read/count` | `readCount("equip")` | 10k | 10k | rust, hx |

Report: **ops/sec** and **p50/p95/p99 latency** across iterations.

`readById` is the baseline: both backends are cache-hits. If rustFolio is slower here
than hxFolio it's pure IPC overhead, which gives us the IPC floor cost.

### 4.3 Write Operations

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

### 4.4 History Operations

hxFolio does not implement the history API. These are rustFolio-only.

| Scenario | Op | Items | Points | Iterations |
|----------|----|-------|--------|------------|
| `his/write_100` | `hisWrite` | 100 | 1 | 1k |
| `his/write_1000` | `hisWrite` | 1000 | 1 | 500 |
| `his/write_10000` | `hisWrite` | 10000 | 1 | 100 |
| `his/write_parallel_100pts` | 100-point concurrent writes | 100/pt | 100 | 10 |
| `his/read_full_1k` | `hisRead` no span | 1k items | 1 | 1k |
| `his/read_full_10k` | `hisRead` no span | 10k items | 1 | 500 |
| `his/read_full_100k` | `hisRead` no span | 100k items | 1 | 100 |
| `his/read_span_10pct` | Span (~10% of range) | 10k items | 1 | 500 |
| `his/stat` | `hisStat` | 10k items | 1 | 10k |

The `his/write_*` series characterizes the incremental stat algorithm under load.  
The `his/read_span_*` validates the before/in-span/after boundary logic at scale.

### 4.5 Mixed Workload (Realistic)

Simulates a running Haxall instance: concurrent reads and writes at a realistic ratio.

| Scenario | Read % | Write % | His % | Duration |
|----------|--------|---------|-------|----------|
| `mixed/read_heavy` | 90 | 5 | 5 | 30s |
| `mixed/write_heavy` | 50 | 40 | 10 | 30s |
| `mixed/his_heavy` | 40 | 10 | 50 | 30s |

Each scenario runs sequentially (not threaded — folio is single-actor) at realistic cadence.
Reports: total ops, ops/sec, error count.

---

## 5. Scalability Matrix

The key question: how does performance scale with record count?

| Metric | 1k recs | 5k recs | 10k recs | 50k recs | 100k recs |
|--------|---------|---------|----------|----------|-----------|
| Startup time | — | — | — | — | — |
| `readAll("equip")` throughput | — | — | — | — | — |
| `readAll("equip and point")` throughput | — | — | — | — | — |
| `commitAll` single-record latency | — | — | — | — | — |
| Memory (RSS) | — | — | — | — | — |

Memory is tracked via `/proc/self/rss` on Linux or `ps` on macOS between operations.

---

## 6. What to Measure

For each benchmark, capture:

| Metric | Unit | Notes |
|--------|------|-------|
| Throughput | ops/sec | Primary metric for bulk operations |
| p50 latency | µs or ms | Typical-case latency |
| p95 latency | µs or ms | Tail latency (folio is synchronous actor) |
| p99 latency | µs or ms | Worst-case outliers |
| Variance | % | High variance indicates GC or OS scheduling jitter |

For Tier 1 (criterion): statistical confidence intervals are automatic.  
For Tier 2 (Fantom): collect raw durations, compute percentiles in the harness.

---

## 7. Expected Outcomes & Hypotheses

| Area | Hypothesis | Confidence |
|------|-----------|------------|
| `readById` | rustFolio slightly slower due to IPC round-trip overhead | High |
| `readAll` simple filter, large dataset | rustFolio faster (Rust filter eval vs. Fantom JVM) | Medium |
| `readAll` compound filter, indexed | rustFolio comparable or faster (tag index intersection in Rust) | Medium |
| `commitAll` single record | rustFolio slower (redb txn overhead + IPC) vs. hxFolio zinc file | Medium |
| `commitAll` batch ≥100 | rustFolio faster (redb batches amortize txn cost; zinc writes N files) | High |
| History write | rustFolio-only; redb should scale linearly with batch size | High |
| Startup with 100k records | rustFolio slightly slower (redb scan vs. zinc scan) | Low |
| IPC floor | readById delta tells us the per-call IPC cost (budget for future ops) | High |

The IPC floor is the most architecturally important number. Every rustFolio operation
pays it. Knowing it tells us whether future optimizations should target Rust-internal
code or the IPC layer.

---

## 8. Implementation Plan

### Phase 1 — Tier 1 Criterion Setup (1 session)

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

### Phase 2 — Tier 2 Fantom Harness (1 session)

```
fan/bench/
└── BenchFolio.fan     // standalone script
```

Structure:
```fantom
class BenchFolio
{
  Void main(Str[] args) { /* parse args, run scenarios, print table */ }
  Void benchReadAll(Folio f, Str filter, Int recs, Int iters) { ... }
  Void benchCommitAll(Folio f, Int batchSize, Int iters) { ... }
  Void benchHisWrite(Folio f, Int items, Int iters) { ... }
  Void printResults() { /* markdown table */ }
}
```

Backends instantiated via the same `FolioConfig` mechanism as `testFolio`:
```fantom
if (backend == "rust") folio = RustFolio.open(config)
if (backend == "hx")   folio = HxFolio.open(config)
```

### Phase 3 — Run & Record (1 session)

1. Run Tier 1: `cargo bench` — capture `target/criterion/` HTML reports
2. Run Tier 2: `fan BenchFolio.fan --backend both --recs 10000 --iters 1000`
3. Run Tier 2 scalability: `--recs 1000,5000,10000,50000,100000`
4. Document results in `BENCHMARKS_RESULTS.md`

---

## 9. Benchmark Data Generation

Records seeded with realistic tag structures:

```fantom
// Typical "equip" record — 8 tags
{id, dis, equip, siteRef, equipRef, ahu, hvac, mod}

// Typical "point" record — 12 tags
{id, dis, point, equip, his, siteRef, equipRef, kind, unit, sensor, cur, writable, mod}
```

History seeded with uniform 1-minute interval ticks, `Number` values.

The seeding pass is excluded from benchmark timing (setup only).

---

## 10. Deliverables

| Artifact | Description |
|----------|-------------|
| `rust/benches/*.rs` | Criterion benchmark files |
| `fan/bench/BenchFolio.fan` | Fantom integration harness |
| `BENCHMARKS_RESULTS.md` | Final results with tables and analysis |
| `target/criterion/` | Criterion HTML reports (local, not committed) |

---

## Notes & Constraints

- Benchmarks run on Trevor's Mac mini (arm64, Apple Silicon) — note CPU/memory in results
- All Tier 2 benchmarks run sequentially; folio is a single-actor model so threading is not relevant
- hxFolio does not support history API — history benchmarks are rustFolio-only
- redb's MVCC model means write transactions may block read transactions briefly; the mixed workload benchmark will surface this if it's a problem
- `cargo bench` requires a `release`-profile Rust binary — the same binary used in production
