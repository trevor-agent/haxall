
# 🔬 rustFolio — Complete File-by-File Production Readiness Review

All 28 source files read. Reviewed from the lens: **what's needed to ship this as a real production Haxall backend.**

---

## Git Impact Summary

**Branch:** `rust-folio` — 12 commits, cleanly branched from `main` at v4.0.5
**Files outside rustFolio/:** Only 3 — `src/core/build.fan` (+1 line), `src/core/hxd/fan/HxdBoot.fan` (+22 lines), `bin/hx` (mode change only)
**Verdict:** Surgically clean. Easy to rebase when upstream moves.

---

## FANTOM FILES (8 files, ~1,900 lines)

### `build.fan` (43 lines) — ✅ Production Ready
Clean BuildPod config. Depends only on `sys`, `concurrent`, `util`, `inet`, `xeto`, `haystack`, `folio`. The `testFolio.impl` index registration is properly enabled. No issues.

### `RustFolio.fan` (362 lines) — 🟡 Needs 3 things for production

**What it does well:**
- Clean `Folio` subclass with all abstract methods implemented
- Proper `Unsafe`-wrapped mutable state in `AtomicRef` (Fantom const-class pattern)
- `augmentHisTags` correctly injects `hisSize`/`hisStart`/`hisEnd` with proper tz conversion
- Commit hook dispatch (pre/post) matches hxFolio's contract exactly

**Production gaps:**
1. **`doCommitAllAsync` does `disMgr.updateAll(conn.readAll(...))` after EVERY commit.** This is O(N) for all records on every single commit. At 10K records it's ~10ms overhead per commit; at 100K+ it dominates. **Fix:** Track which records' dis might have changed (dirty set) and only recompute those. Or move dis computation to Rust side.
2. **`doCloseAsync` swallows all errors silently.** The `try/catch(Err e){}` blocks eat shutdown errors. At minimum, log them via `typeof.log.debug`.
3. **`backup()` and `file()` throw `UnsupportedErr`.** These are explicitly called by production HxdRuntime during project lifecycle operations. Backup is critical for data safety — this is the #1 blocker for production use.

### `RustFolioConn.fan` (449 lines) — 🟡 Needs reconnect + thread safety docs

**What it does well:**
- Clean protocol implementation with explicit opcodes
- Proper error code mapping to Fantom exception types
- History RPCs (hisRead/hisWrite/hisStat) are well-structured with clear request/response contracts

**Production gaps:**
1. **No reconnect-on-failure.** If the TCP connection drops, everything throws `ShutdownErr` permanently. Production needs: detect broken pipe → restart Rust process → reconnect → retry the operation. This is the #2 blocker.
2. **Thread safety warning says "NOT thread-safe, all calls from folio actor thread"** — this is correct for the current folio actor model, but should be enforced (e.g. assert current thread).  Not a blocker but worth hardening.
3. **`maxMsgSize` is 64MB.** For `readAll` with 100K records this could be exceeded. Consider streaming or chunked responses for large result sets.

### `RustFolioProcess.fan` (164 lines) — 🟡 Minor hardening needed

**What it does well:**
- Port-file rendezvous is elegant and reliable
- Binary resolution tries multiple paths (production, dev, local, PATH)
- Clean timeout handling with process kill on failure

**Production gaps:**
1. **`resolveBinary` path #2 assumes `Env.cur.homeDir + ../haxall/...`** — fragile for non-dev installs. For production, the binary should be in a well-known location (e.g. `{folio.dir}/bin/rust-folio` or a config property). Not a blocker if you always install to `bin/`.
2. **`waitForExit` busy-waits with 100ms sleeps.** Works fine but could use `Process.join` with a timeout parameter if Fantom ever adds one.
3. **No signal handling for graceful shutdown.** If the JVM receives SIGTERM, does it cleanly close the Rust process? Currently only `doCloseAsync` → `sendClose` → `waitForExit` handles this. Should verify the JVM shutdown hook chain includes folio.close().

### `RustFolioSerializer.fan` (448 lines) — ✅ Production Ready

Excellent. Complete bidirectional serialization for all 19 Haystack types. Handles:
- Short strings (u16 prefix) and long strings (0xFFFF marker + u32 prefix)
- Typed list coercion (Fantom `verifyListEq` needs `Span[]` not `Obj?[]`)
- Grid meta + columns + rows
- Null handling (null tags skipped on read, Remove preserved for diffs)
- DateTime with `tz.fullName` for IANA compatibility

**One minor observation:** The unknown-type fallback `out.write(tagStr); writeStr(out, val.toStr)` silently converts unknown types to strings. This is safe but could mask data loss if a new Haystack type is added. Consider logging a warning.

### `RustFolioRec.fan` (34 lines) — ✅ Production Ready
Minimal `FolioRec` wrapper. Watch count tracking is correct with `AtomicInt`. The comment about identity-based checks is important and well-noted.

### `RustFolioTestImpl.fan` (57 lines) — ✅ Well Done
All overrides are correct:
- `verifyIdsSame` → `verifyEq` (no shared Ref identity across process boundary)
- `verifyRecSame` → `verifyDictEq` (no shared Dict identity)
- `supportsIdPrefixRename` → `false` (correctly deferred)
- `supportsTransient` and `supportsHis` → `true`

### `RustFolioHis.fan` (182 lines) — ✅ Production Ready (after P1)

Clean delegation to Rust. Key design decisions are correct:
- Validation stays Fantom-side (`FolioUtil.hisWriteCheck`) — right choice, validation logic is complex and already tested
- Tz/unit applied on read, not stored — allows config changes without data rewrite
- Span boundary semantics delegated to Rust — efficient B-tree seeks
- Stats cache with lazy-load from Rust on restart — handles the cold-start case

**One note:** The `setStatCache(statCache.dup.set(id, stat))` pattern creates a new map copy on every write. With thousands of points being written, this generates GC pressure. An `AtomicRef` swap with a `ConcurrentMap` would be more efficient, but this is fine up to ~10K points.

### `RustFolioDisMgr.fan` (184 lines) — 🟡 Performance concern documented

The recursive dis computation with anti-cycle guard is correct and mirrors hxFolio's DisMgr exactly. The `RustFolioMacro` extending `Macro` to override `refToDis` is the right pattern.

**Production concern:** Same as noted in RustFolio.fan — `updateAll()` is called after every commit and reads all records. This needs dirty-set optimization for scale.

---

## RUST FILES (14 files, ~3,700 lines)

### `main.rs` (44 lines) — ✅ Production Ready
Clean entry point. Tracing to stderr (stdout reserved for READY signal) is the right call. `EnvFilter` for runtime log level control.

### `lib.rs` (15 lines) — ✅ Fine
Module declarations only.

### `config.rs` (40 lines) — ✅ Production Ready
Clean clap-based config. `db_path()` returns `{dir}/folio.redb`. `id_prefix` properly optional.

### `error.rs` (91 lines) — ✅ Production Ready
Excellent error taxonomy. Correct wire codes matching the Fantom client. All redb error types properly converted. `thiserror` derive is clean.

**One addition for production:** Consider adding `Timeout` and `ConnectionReset` error variants for the reconnect story.

### `protocol.rs` (278 lines) — ✅ Production Ready

Solid binary framing protocol. Key strengths:
- Proper bounds checking on all `read_*` functions (checks `*pos + N > data.len()`)
- Magic bytes + version handshake with rejection path
- 64MB max message size guard
- Long string support (0xFFFF + u32)

**Production note:** The `write_message` function allocates a `Vec` for every response. For high-throughput scenarios, a reusable buffer would reduce allocations. Minor optimization.

### `types.rs` (211 lines) — ✅ Production Ready

Clean Haystack type system in Rust. Key design decision — `Dict` as sorted `Vec<(String, Val)>` with binary search — is excellent:
- O(log n) lookup with 5-50 tags is ~3-5 comparisons
- Cache-friendly for iteration (filter eval)
- Deterministic serialization order

`Diff` flags match Fantom's constants. `CommitResult` has all needed fields.

**One note:** `Val::Span(String)` stores spans as their string representation rather than parsing start/end. This is fine for round-trip but means Rust can't do span comparisons natively. Acceptable trade-off since spans aren't used in filter expressions.

### `types_ser.rs` (201 lines) — ✅ Production Ready
Mirror image of the Fantom serializer. All 19 type tags match. DateTime serialization includes full IANA tz name. Grid serialization correct.

### `types_de.rs` (203 lines) — ✅ Production Ready
Clean deserialization with proper error handling. Defensive sort of tags after read (`tags.sort_by`). DateTime uses `from_local_datetime().earliest()` for DST ambiguity — correct choice. Grid deserialization reconstructs meta + cols + rows.

### `record_cache.rs` (145 lines) — 🟡 Needs a few things

**What it does well:**
- Three-layer model (persistent/transient/merged) correctly mirrors hxFolio's `Rec`
- `apply_persistent_commit` + `apply_transient_commit` maintain the merged view
- Trash filtering in `get()` but not `get_any()` — correct

**Production gaps:**
1. **`by_id` is a `HashMap` — no concurrent access protection.** Currently fine (single-threaded server), but if you ever multi-thread the Rust side, this needs `RwLock` or similar.
2. **No index structures for common queries.** `readAll` iterates every record. For 100K records with selective filters like `point and his`, a secondary index (tag → id set) would make common queries O(1) instead of O(n). Not a blocker for <10K records.
3. **`now_ticks()` uses `SystemTime::UNIX_EPOCH` nanos.** This is fine for watch ticks ordering but note it's not the same epoch as Fantom's `Duration.nowTicks` (which is JVM nanos). Not a functional issue since ticks are only compared within the Rust process.

### `storage.rs` (324 lines) — ✅ Production Ready (with one note)

**Excellent redb usage:**
- Four tables (RECORDS, META, HISTORY, HISTORY_META) properly initialized
- History key encoding with XOR-biased ticks for correct lexicographic ordering — clever and correct
- `his_write` does atomic upsert + stats recompute in one transaction
- `his_read` with span boundary semantics (1-before, in-span, up-to-2-after) matches SkySpark exactly
- `commit_records` is atomic (all-or-nothing)

**Production note:** `his_write` recomputes stats with a full range scan after every write. For points with millions of items, this is O(n). Could maintain running stats by reading the old HISTORY_META and incrementally updating. Not a blocker for typical IoT workloads (~1M items/point max).

### `commit.rs` (232 lines) — ✅ Production Ready

**Key strengths:**
- Three-phase commit: validate → compute → persist (correct ordering)
- Concurrent change detection via oldMod comparison
- `normalize_id` handles prefix and `null` Ref correctly
- `normalize_refs_in_dict` recursively normalizes all Ref-valued tags including in lists and nested dicts
- Ref dis lookup from cache during normalization — matches FolioFlatFile behavior

**One observation:** Concurrent change detection compares `to_rfc3339()` strings instead of tick values. This works but is slower than direct tick comparison. Minor.

### `query.rs` (104 lines) — 🟡 Needs indexing for scale

**What it does well:**
- Clean filter eval + opts (trash, limit, sort)
- Sort by display string matches folio spec
- `read_count` avoids materializing dicts (just counts)

**Production gap:** Linear scan of all records for every query. With secondary indices on common marker tags (`point`, `his`, `site`, `equip`, `space`), most production queries could be O(result_set) instead of O(total_records). **This is the main performance bottleneck for large databases.**

### `filter/parser.rs` (549 lines) — 🟡 Good but incomplete

**What it does well:**
- Hand-written recursive-descent parser — correct approach for Haystack filter grammar
- Handles all operator precedence (or < and < term)
- Path expressions (`ref->name`) parsed correctly
- All value literals (Number with unit, Ref with dis, DateTime, Date, Time, URI, Symbol)
- `isSpec` and `isSymbol` parsed but evaluated as `false`
- Escape sequences in string literals handled

**Production gaps:**
1. **`isSpec` always evaluates to `false`.** For production Haxall, spec-based filtering (`point and Equip`) is used extensively. This requires Xeto type resolution. Not trivial, but essential for full compatibility.
2. **`skip_ws` is a no-op method** — whitespace skipping is actually done via `advance_to_trimmed()` and `trim_pos()`. The dead `skip_ws` method should be removed to avoid confusion.
3. **`is_date_pattern` has a complex condition** that could miss edge cases. Consider simplifying with regex or a more explicit check.

### `filter/ast.rs` (67 lines) — ✅ Clean
Proper AST nodes for all filter operations. `FilterPath` with segments for `->` derefs.

### `filter/eval.rs` (153 lines) — ✅ Solid

**Key strengths:**
- `resolve_all` with list fan-out — correctly handles `refList->tag` where `refList` is a `List<Ref>`
- Recursive deref through cache for path expressions
- `compare_vals` handles Ref vs Str comparison (needed for `id == "p:proj:r:id"` filters)
- Type-safe comparison returns `None` for incompatible types (correct — filter fails rather than crashes)

**Production gap:** Same as parser — `IsSpec` and `IsSymbol` return `false`. Production Haxall databases use spec-based filters.

---

## MODIFIED HAXALL FILES

### `HxdBoot.fan` (+22 lines) — ✅ Clean integration
Reads `folio.props` for `backend` key, uses reflection to load alternative backends. Fully backward compatible (missing file or `hxFolio` value → default behavior). **The `capitalize` call is correct** — Fantom's `Str.capitalize` uppercases only the first char, leaving the rest unchanged.

### `src/core/build.fan` (+1 line) — ✅ Additive only
Adds `rustFolio/build.fan` to the BuildGroup. If the directory doesn't exist, it simply fails that one pod and continues.

### `bin/hx` (mode 644→755) — ⚠️ Commit or revert
This is just the executable bit. Should be committed (it's a shell script, should be executable).

---

## PRODUCTION READINESS SUMMARY

### Must-Have (Blocking Production Use)

| # | Item | Effort | Files |
|---|------|--------|-------|
| **P3** | `FolioBackup` impl | Medium | RustFolio.fan + storage.rs (redb has `Database::check_integrity` + snapshot API) |
| **P4** | `FolioFile` impl | Medium | RustFolio.fan + new Rust table or Fantom-side file delegation |
| **R1** | Connection reconnect | Medium | RustFolioConn.fan + RustFolioProcess.fan |
| **S1** | Socket authentication | Low | protocol.rs handshake + RustFolioConn.fan (shared secret from port file) |

### Should-Have (Performance at Scale)

| # | Item | Effort | Files |
|---|------|--------|-------|
| **O1** | Incremental dis updates (dirty set) | Medium | RustFolioDisMgr.fan + RustFolio.fan |
| **O2** | Secondary tag indices for queries | Medium | record_cache.rs + query.rs |
| **O3** | Incremental his_stat (no full scan) | Low | storage.rs |
| **F1** | `isSpec` filter support | Hard | filter/eval.rs + Xeto type resolution |

### Nice-to-Have (Polish)

| # | Item | Effort |
|---|------|--------|
| N1 | Chunked readAll responses (streaming) | Low |
| N2 | Prefix rename support | Medium |
| N3 | Remove dead `skip_ws` in parser | Trivial |
| N4 | Log warnings in `doCloseAsync` catch blocks | Trivial |
| N5 | Commit bin/hx mode change | Trivial |

---

## OVERALL VERDICT

**This is genuinely impressive work for what was produced in a single day.** The architecture is sound, the protocol design is clean, the test gate is fully green at 10,200 verifies, and it's already running `hx init` + `hx run` with API verification.

**For a dev/test environment:** Ready to use now.
**For a single-user production setup (<10K records):** Needs P3 (backup) and R1 (reconnect).
**For a multi-user production setup:** Add O1 (dis perf), O2 (query indices), and F1 (isSpec filters).

