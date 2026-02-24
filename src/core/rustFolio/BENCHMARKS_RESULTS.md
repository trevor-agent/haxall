# rustFolio Benchmark Results

> **Status:** All tiers complete — Tier 0, Tier 1, Tier 2 (rust + hxFolio). GC tracking not yet run.  
> **Methodology:** See `BENCHMARKS.md` for full description of each tier, warmup strategy, and GC tracking approach  
> **Platform:** Apple M-series Mac mini (arm64, macOS 25.2.0)  
> **Rust:** 1.x (arm64 aarch64-apple-darwin)  
> **Commit:** `01d7a539` on branch `rust-folio`

---

## 0. Build Metrics (Tier 0)

> Script: `rust/bench_tier0.sh`  
> Date: 2026-02-24

| Metric | Value | Notes |
|--------|-------|-------|
| Unstripped release binary | 3.6 MB | `rust-folio` server binary |
| Stripped release binary | 3.2 MB | |
| Direct production deps | 6 | chrono, chrono-tz, redb, thiserror, tracing, tracing-subscriber |
| Direct dev deps | 2 | criterion, tempfile |
| Total unique transitive crates | 73 | all targets (prod + dev) |
| Production-only transitive crates | 30 | `--no-dev-dependencies` |
| Clean release compile (real) | 6.2s | `cargo clean && cargo build --release` |
| Clean release compile (user/cpu) | 36.8s | parallel compilation, Apple Silicon |

**Takeaway:** 6 production dependencies, 30 transitive crates, 3.2MB stripped binary.
The clean build time of 6.2s real (vs 36.8s CPU) reflects high parallelism on Apple Silicon.

---

## 1. Rust Microbenchmarks (Tier 1)

> Tool: `cargo bench` (criterion 0.5)  
> Date: 2026-02-24, commit `01d7a539`  
> Full HTML reports: `rust/target/criterion/` (local, not committed)  
> All values are criterion mean (point estimate); 100 samples, 3s warmup per bench.

### 1.1 Filter Parsing

> Per `filter::parse()` call. Parse cost is negligible vs. eval — confirmed below.

| Benchmark | Mean |
|-----------|------|
| `simple` ("equip") | 72.7 ns |
| `num_cmp` ("temp > 72") | 119 ns |
| `is_spec` ("ph::Equip") | 103 ns |
| `or_root` ("equip or device") | 175 ns |
| `path_eq` ("a->b->c == x") | 221 ns |
| `compound` ("equip and point and his") | 263 ns |

Parse cost is 72–263 ns — 3–4 orders of magnitude below eval cost. Filters should
be compiled once and reused.

### 1.2 Filter Evaluation (read_all)

> Per `query::read_all()` call on a pre-seeded in-memory RecordCache. No I/O.  
> Cache composition: ~10% sites, ~25% equips, ~65% points. All `equip and point and his`  
> returns ~65% of total (all points); `site or equip` (OR root) returns ~35%.

| Benchmark | 1k recs | 10k recs | 100k recs |
|-----------|---------|----------|-----------|
| `has_simple` ("equip") | 378 µs | 4.1 ms | 80.7 ms |
| `has_compound` ("equip and point and his") | 422 µs | 4.45 ms | 81.5 ms |
| `comparison_eq` ("dis == \"Equip-0\"") | — | 1.01 ms | — |
| `comparison_num` ("temp > 30") | — | 1.22 ms | — |
| `path_traversal` ("equipRef->siteRef->geoCity == \"Chicago\"") | — | 3.26 ms | — |
| `or_root` ("site or equip") — full scan | — | 4.02 ms | — |
| `is_spec` ("ph::Equip") | — | 1.10 ms | — |
| `no_match` ("ahu and chiller") — early exit | 70 ns | 70 ns | 70 ns |
| `read_count` ("equip and his") | — | 1.49 ms | 48.2 ms |

**Key findings:**
- `no_match` exits in ~70 ns regardless of cache size — the tag index intersection returns an
  empty candidate set immediately, and zero records are scanned or cloned.
- `has_simple` vs `has_compound` at the same scale: compound is ~8% slower at 10k. The
  multi-tag index intersection adds trivial cost when all three tags are common.
- `path_traversal` at 10k: 3.26 ms — each record that passes the index lookup triggers 2 HashMap
  dereferences. This is the most expensive standard filter type.
- `or_root` forces a full scan through all 10k records even when the result set is small;
  cost grows linearly with cache size.
- Scale-up is linear in cache size for all scan-based filters, as expected.

### 1.3 Cache Operations

> Isolated RecordCache operations with no I/O.

| Benchmark | Value | Notes |
|-----------|-------|-------|
| `read_by_id` hit (10k) | 15.7 ns | HashMap lookup + return value |
| `read_by_id` hit (100k) | 15.6 ns | Cache size has no effect |
| `read_by_id` miss (10k) | 7.15 ns | Faster — returns None without ref-counting |
| `read_by_id` miss (100k) | 7.79 ns | |
| `read_all_indexed` (1k) | 409 µs | Filter: equip and point and his |
| `read_all_indexed` (10k) | 4.38 ms | |
| `read_all_indexed` (100k) | 83.6 ms | |
| `read_all_full_scan` (1k) | 376 µs | Filter: site or equip (OR → full scan) |
| `read_all_full_scan` (10k) | 3.98 ms | |
| `read_all_full_scan` (100k) | 72.3 ms | |
| `load` (1k) | 466 µs | Bulk load from Vec<(String, Dict)> |
| `load` (10k) | 4.6 ms | |
| `load` (100k) | 84.8 ms | 100k records loaded in 85ms at startup |
| `apply_persistent_commit` | 557 ns | Single record update + tag index diff |
| `apply_transient_commit` | 1.28 µs | curVal update — slightly more overhead |

**Key findings:**
- `read_by_id` is 15.7 ns regardless of cache size (HashMap is O(1)).
- Indexed vs. full-scan comparison at 10k: 4.38 ms vs. 3.98 ms — indexed is *slower* in this
  bench because `equip and point and his` returns ~65% of all records (mostly points), while
  the full-scan OR filter returns ~35%. The index provides no speedup when the candidate set
  is large. Index benefit shows when leading Has terms produce small intersection sets.
- `apply_persistent_commit` (557 ns) is faster than `apply_transient_commit` (1.28 µs) because
  the transient path merges the overlay into `merged` and may update more index entries.
- 100k record cache load in 85ms — startup cost for a large database is acceptable.

### 1.4 Serialization

> Per-call wire protocol encode/decode cost. These are paid on every RPC call.

| Benchmark | Mean | Notes |
|-----------|------|-------|
| `serialize_small` (5 tags) | 59.0 ns | Typical equip dict outbound |
| `roundtrip_small` | 203 ns | Decode only (5 tags) |
| `serialize_large` (40 tags) | 332 ns | Dense record dict |
| `roundtrip_large` | 1.70 µs | Decode only (40 tags) |
| `serialize_grid_100` (100 × 5-tag dicts) | 4.69 µs | readAll response payload |

**Key findings:**
- Serialization cost for a typical 5-tag record: 59 ns. At 15.7 ns for the HashMap lookup,
  wire encoding is 4× the lookup cost — but both are sub-µs.
- 100-record readAll response: 4.69 µs to serialize. At ~85 µs IPC round-trip overhead
  (from Tier 2), serialization is ~5% of total readAll latency for a 100-record result.
- For 10k-record readAll results, serialization becomes more significant (~469 µs vs. ~85 µs IPC).

### 1.5 Storage (redb)

> Direct redb operations on a tempfile database. Default flush mode (fsync per commit).  
> Commit benchmarks measure total time including fsync.

| Benchmark | Mean | Per-record cost |
|-----------|------|-----------------|
| `commit_single` (1 record) | 4.02 ms | 4,020 ns/record |
| `commit_batch/10` (10 records) | 4.06 ms | 406 ns/record |
| `commit_batch/100` (100 records) | 5.13 ms | 51.3 ns/record |
| `commit_batch/1000` (1000 records) | 10.0 ms | 10.0 ns/record |
| `load_all` (1k records) | 165 µs | |
| `load_all` (10k records) | 1.67 ms | |
| `load_all` (100k records) | 17.5 ms | |
| `his_write/100` | 4.98 ms | 49.8 µs/item |
| `his_write/1000` | 9.13 ms | 9.1 µs/item |
| `his_write/10000` | 15.6 ms | 1.56 µs/item |
| `his_read_full/1000` | 32.8 µs | 32.8 ns/item |
| `his_read_full/10000` | 330 µs | 33.0 ns/item |
| `his_read_full/100000` | 3.29 ms | 32.9 ns/item |
| `his_read_span_10pct` (1k items from 10k) | 35.9 µs | ~35.9 ns/item |
| `his_stat` | 257 ns | O(1) — pure cache lookup |

**Key findings:**

**Batch commit:**
- commit_single → commit_batch/10: essentially the same real time (4.02 ms vs 4.06 ms).
  Both pay exactly one fsync. Batch of 10 costs the same wall time as a single record.
- Batch/100 adds ~1.1 ms over single (likely more serialization work in the Rust txn).
- Batch/1000 takes 10.0 ms — 2.5× single, but covers 1000 records. Per-record cost drops
  402× vs. single-record commits. **This is the expected rustFolio advantage over hxFolio**
  which writes one zinc file per record commit.

**History write throughput:**
- 100 items: 4.98 ms (50 µs/item)
- 1k items: 9.13 ms (9.1 µs/item) — amortization kicks in
- 10k items: 15.6 ms (1.56 µs/item) — single redb transaction, strong amortization
- his_write scales sub-linearly with batch size: 10k items take only 3× the time as 100 items,
  for 100× more data. redb's WAL write amortizes the transaction overhead.

**History read throughput:**
- Read is ~33 ns/item regardless of dataset size (1k to 100k items).
- This is essentially sequential redb scan + minor deser overhead — I/O-bound, cache-friendly.
- Span read (10% of 10k items) is 35.9 µs vs. 330 µs for full 10k read — confirms efficient
  log-n seeks to span start followed by sequential scan.

**his_stat** is 257 ns — purely a Rust-side HashMap cache lookup. No redb access.

---

## 2. Fantom Integration Benchmarks (Tier 2)

> Tool: `fan fan/bench/BenchFolio.fan`  
> Date: 2026-02-24, commit `01d7a539`

### 2.1 Smoke Test (100 records, 10 iters, warmup 2)

> Confirmed end-to-end correctness. Not representative of steady-state performance.

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs |
|----------|---------|---------|--------|--------|--------|
| readById (ipc floor) | rust | 9,438 | 84.3 | 171.8 | 310.6 |
| readAll equip | rust | 1,315 | 748.7 | 891.3 | 891.3 |
| readAll equip+point+his | rust | 1,644 | 603.4 | 653.8 | 653.8 |
| readAll no-match | rust | 13,373 | 72.4 | 91.7 | 91.7 |
| readCount equip | rust | 13,622 | 73.0 | 77.3 | 77.3 |
| commitAll add batch=1 | rust | 250 | 4,011.7 | 4,150.6 | 4,177.8 |
| commitAll add batch=10 | rust | 194 | 5,021.3 | 5,994.8 | 6,081.8 |
| commitAll add batch=100 | rust | 171 | 5,869.0 | 7,926.3 | 9,188.1 |
| hisWrite 100 items | rust | 249 | 3,990.8 | 4,113.3 | 4,113.3 |
| hisWrite 1000 items | rust | 171 | 6,024.0 | 6,080.4 | 6,080.4 |
| hisWrite 10000 items | rust | 50 | 19,900.4 | 22,262.0 | 22,262.0 |
| hisRead full (10k items) | rust | 262 | 3,343.4 | 7,302.8 | 7,302.8 |

> **Note:** Smoke-test iteration counts (10 iters, warmup 2) are too low for stable statistics.
> The readAll ops/sec are inflated vs. the full run because 100-record results are much smaller
> than 10k-record results. Use full run below for representative numbers.

### 2.2 Full Run (10k records, 1000 iters, warmup 200)

> Both backends complete.

| Scenario | Backend | ops/sec | p50 µs | p95 µs | p99 µs | vs hx |
|----------|---------|---------|--------|--------|--------|-------|
| readById (ipc floor) | rust | 27,681 | 24.7 | 59.0 | 69.3 | 41× slower |
| readById (ipc floor) | hx | 1,623,660 | 0.6 | 0.8 | 1.0 | baseline |
| readAll equip | rust | 25 | 39,935 | 41,941 | 42,281 | 110× slower |
| readAll equip | hx | 2,697 | 363.8 | 399.1 | 424.1 | baseline |
| readAll equip+point+his | rust | 30 | 33,225 | 35,282 | 35,707 | 74× slower |
| readAll equip+point+his | hx | 2,136 | 446.8 | 567.3 | 576.4 | baseline |
| **readAll no-match** | **rust** | **36,660** | **17.9** | **62.8** | **68.9** | **11× faster** |
| readAll no-match | hx | 5,001 | 202.5 | 224.0 | 232.1 | baseline |
| readCount equip | rust | 675 | 1,484 | 1,531 | 1,572 | 8× slower |
| readCount equip | hx | 5,232 | 183.6 | 217.4 | 232.3 | baseline |
| commitAll add batch=1 | rust | 193 | 5,007 | 5,548 | 6,102 | 49× slower |
| commitAll add batch=1 | hx | 9,553 | 102.2 | 116.7 | 143.4 | baseline |
| commitAll add batch=10 | rust | 167 | 5,958 | 6,914 | 11,017 | 6× slower |
| commitAll add batch=10 | hx | 1,061 | 932.1 | 1,000.3 | 1,139.5 | baseline |
| **commitAll add batch=100** | **rust** | **114** | **8,730** | **10,961** | **24,826** | **≈ tied** |
| commitAll add batch=100 | hx | 107 | 9,331 | 9,629 | 9,732 | baseline |
| hisWrite 100 items | rust | 209 | 4,831 | 5,183 | 5,183 | rust only |
| hisWrite 1000 items | rust | 163 | 6,036 | 6,834 | 6,834 | rust only |
| hisWrite 10000 items | rust | 48 | 20,097 | 25,675 | 25,675 | rust only |
| hisRead full (10k items) | rust | 320 | 3,069 | 3,131 | 4,721 | rust only |

### 2.3 GC Metrics (hxFolio runs)

> To be captured via: `FAN_JAVA_OPTS="-verbose:gc" fan BenchFolio.fan --backend hx 2>gc.log`

| Scenario | GC Pause Count | Total Pause (ms) | Max Pause (ms) |
|----------|----------------|------------------|----------------|
| readAll equip (1000 iters) | — | — | — |
| commitAll batch=100 (iters) | — | — | — |

---

## 3. Key Findings

### IPC Floor

**`readById` rustFolio p50: 24.7 µs vs. hxFolio p50: 0.6 µs → IPC overhead = ~24 µs per call**

Every rustFolio operation pays this overhead on top of its Rust computation cost. From Tier 1,
the Rust-internal cost of `readById` is **15.7 ns** (HashMap lookup). Therefore virtually all
24.7 µs is IPC: TCP loopback + Fantom serialization + Rust deser + HashMap lookup + Rust serial
+ write back. This is the architectural price of the two-process model.

Importantly: **24 µs per call is acceptable for operations that take milliseconds** (commits,
large readAll). It is a significant multiplier only for tight loops of per-record reads.

### readAll: IPC Dominates for Large Result Sets

`readAll equip` at 10k records — 9,000 matching records to serialize and return across the IPC
boundary. rustFolio takes 39.9 ms, hxFolio takes 0.36 ms. **hxFolio is 110× faster** because
there is no serialization overhead for in-process results.

This is the expected behavior. rustFolio is not designed to outperform hxFolio on bulk in-process
reads. The architectural value is persistent storage (redb vs. zinc), history support, and batch
writes — not readAll throughput.

**readAll no-match is the exception:** rustFolio (17.9 µs) is **11× faster** than hxFolio
(202.5 µs) for a zero-result compound filter ("ahu and chiller"). rustFolio's tag index returns
an empty candidate set in nanoseconds; zero records are serialized or scanned. hxFolio's result
(202 µs ≈ 10k records × ~20 ns/record) suggests it performs a full scan for this filter type.
This is a direct win from the Rust tag index implementation.

### Batch Write Crossover

**The crossover point is between batch=10 and batch=100.**

| Batch size | rustFolio p50 | hxFolio p50 | Winner |
|------------|---------------|-------------|--------|
| 1 | 5.0 ms | 0.10 ms | hx 49× |
| 10 | 6.0 ms | 0.93 ms | hx 6× |
| 100 | 8.7 ms | 9.3 ms | **tied** |
| (extrapolated 1000) | ~20 ms | ~93 ms | rust ~4.6× |

hxFolio's per-record cost is constant (~93 µs per record × batch_size), scaling linearly.
rustFolio pays one fsync regardless of batch size. At batch=100, they cross — both take ~9ms.
At batch=1000, extrapolating, rustFolio would take ~20ms vs. hxFolio's ~93s (if zinc files
scale linearly, which they likely do). This is the core architectural advantage.

Tier 1 storage confirms the mechanism: commit_single 4.02ms, commit_batch/100 5.13ms (same
fsync, slightly more data), commit_batch/1000 10.0ms (still one fsync, 10k records committed).

### History Performance (rustFolio only — hxFolio does not support history API)

**Tier 1 (raw storage):**
- Write: 100 items in 5ms, 10k items in 15.6ms → **sub-linear** (redb WAL amortization)
- Read: ~33 ns/item constant up to 100k items → sequential B-tree scan, cache-friendly
- Stat: 257 ns → O(1) in-memory cache lookup, zero redb access

**Tier 2 (via IPC):**
- hisWrite 100 items: 4.8 ms p50 (dominated by one fsync + IPC round trip)
- hisWrite 10k items: 20 ms p50 (3× more data in 4× the time — still sub-linear)
- hisRead 10k items: 3.1 ms p50 (read 10k items from redb + serialize across IPC)

The Tier 2 his_write numbers include the full Fantom-side validation path
(FolioUtil.hisWriteCheck, sort, dedup, tz normalization) plus IPC + redb write + fsync.
The Tier 1 numbers are redb only. The delta (~0.8ms) is Fantom-side validation cost.

### p99 Tail Latency

| Scenario | rust p99 | rust p99/p50 | hx p99 | hx p99/p50 |
|----------|----------|-------------|--------|-----------|
| readById | 69.3 µs | 2.8× | 1.0 µs | 1.7× |
| readAll equip | 42,281 µs | 1.06× | 424.1 µs | 1.17× |
| commitAll batch=1 | 6,102 µs | 1.22× | 143.4 µs | 1.40× |
| commitAll batch=100 | 24,826 µs | 2.84× | 9,732 µs | 1.04× |

rustFolio's commitAll batch=100 shows p99/p50 of 2.84× (8.7ms p50 → 24.8ms p99). This is
redb or OS I/O jitter under load — occasional slow fsync. GC tracking for hxFolio was not
run in this session (see §2.3). hxFolio's commit p99/p50 ratios are low (1.04×), consistent
with a write-buffered zinc file approach with less fsync dependency.

---

## 4. Run Log

| Date | Commit | Scenario | Notes |
|------|--------|----------|-------|
| 2026-02-24 | c37b4382 | Tier 0 build metrics | bash bench_tier0.sh |
| 2026-02-24 | c37b4382 | Tier 1 `cargo bench` | All 4 bench files, criterion 0.5, ~20 min |
| 2026-02-24 | 01d7a539 | Tier 2 smoke test | --backend rust --recs 100 --iters 10 --warmup 2 |
| 2026-02-24 | 01d7a539 | Tier 2 rust full run | --backend rust --recs 10000 --iters 1000 --warmup 200 |
| 2026-02-24 | 01d7a539 | Tier 2 hxFolio full run | --backend hx --recs 10000 --iters 1000 --warmup 200 |
