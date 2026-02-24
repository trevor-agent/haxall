# rustFolio Implementation Progress

Reference design: `/Users/trevoradelman/Documents/ClineProjects/haxall-rust-dev/`

## Build Environment

- **Workspace:** `~/.openclaw/workspace/fan-dev/`
- **Haxall branch:** `rust-folio` (branched from `main` at 4.0.5)
- **Rust:** 1.93.1
- **Fantom/Haxall:** 4.0.5
- **Build:** `~/.openclaw/workspace/fan-dev/fan/bin/fan src/core/rustFolio/build.fan compile`
- **Test gate:** `~/.openclaw/workspace/fan-dev/fan/bin/fant testFolio`

## Milestone Status

| Milestone | Status | Date | Gate Result |
|-----------|--------|------|-------------|
| M0 - Scaffolding | ✅ complete | 2026-02-23 | Rust crate compiles; Fantom pod compiles; 7 types / 23 methods / 7477 verifies (all green) |
| M1 - Basic CRUD | ✅ complete | 2026-02-23 | testBasics, testReadOpts, testTrash, testFolioFuture, testHooks, testKinds, testRemoveTags all green |
| M2 - Filters | ✅ complete | 2026-02-23 | testFilters, PrefixTest green; full gate 9921 verifies ALL GREEN |
| M3 - Transient | ✅ complete | 2026-02-23 | Transient infra was already complete in Rust; enabled supportsTransient() — gate still ALL GREEN (9906 verifies) |
| M4 - Hooks | ✅ complete | 2026-02-23 | pre/post commit hook dispatch with cxInfo — included in M1/M2 gate |
| M5 - History | ✅ complete | 2026-02-23 | HisTest.testBasics + HisTest.testConfig green; full gate 10164 verifies ALL GREEN |
| M6 - Display | ✅ complete | 2026-02-23 | DisTest green; full gate 10195 verifies ALL GREEN |
| M7 - Full Green | ✅ complete | 2026-02-23 | No deferred no-ops remain in RustFolioTestImpl; gate fully clean |

## Deviations from Reference Design

### DEV-001 — Index registration approach
**Reference:** The reference project notes that the `testFolio.impl` index entry in `build.fan`
should be deferred to M2 (commented out), and `testFolio/build.fan` should not add `rustFolio`
to its depends until M2. The reasoning: `AbstractFolioTest.runImpls` iterates all impls via
`each{}` without per-impl exception isolation, so stub UnsupportedErrs propagate and prevent
`flatfile`/`hx` from running in the same test method.

**This implementation:** Follows the reference exactly. Index registration is commented out in
`build.fan`. Will be enabled at M2 once commits+reads are functional.

### DEV-003 — TCP transport instead of Unix domain sockets
**Reference:** Protocol design assumed Unix domain sockets.
**This implementation:** Fantom's `Socket` class is TCP-only. Using loopback TCP (127.0.0.1)
with an ephemeral port. Rust writes `READY:{port}` to stdout after bind; Fantom reads it from
the process output stream. Security equivalent (loopback-only, same machine).

### DEV-004 — M4 (hooks) implemented with M1/M2
Hook dispatch (pre/post commit with `cxInfo`) was straightforward to add alongside the commit
path. Implemented `RustFolioCommitEvent` and full `FolioHooks` dispatch in `doCommitAllAsync`.
No separate M4 milestone needed; `testHooks` passes as part of the M2 gate.

### DEV-005 — M6 (disMacro/syncDis) — resolved at M6
`DisTest` exercises `disMacro` pattern evaluation and `syncDis` propagation through ref chains.
Initially deferred as no-ops. Fully implemented at M6 via `RustFolioDisMgr` (see DEV-007).
No no-ops remain in `RustFolioTestImpl`.

### DEV-006 — Ref normalization with dis lookup
During commit, all Ref-valued tags in changes are normalized to absolute form. Relative Refs
that have no dis (e.g. `Ref("a")` under prefix `u:`) have their dis populated by looking up
the referenced record in the cache — matching FolioFlatFile's behaviour of setting
`newRec.id.disVal = newRec.dis` and preserving dis through `toRel`/`toAbs` round-trips.
`Ref.nullRef` (id = `"null"`) is explicitly exempt from prefix normalization.

### DEV-007 — M6 dis propagation: full-sweep vs. shared Ref objects
**Reference:** hxFolio uses shared in-memory `Rec` objects. `DisMgr.update(rec)` sets `rec.id.disVal` immediately and kicks off `updateAll` asynchronously. Because all dicts sharing a given record's `Ref` use the same object, the update is instantly visible everywhere.

**This implementation:** Rust deserializes a fresh `Dict` per `readById` call — there are no shared Refs. `RustFolioDisMgr` maintains a `Str:Str` id→dis cache (an `AtomicRef<Unsafe<Map>>`).
- After every commit, `disMgr.updateAll(conn.readAll(...))` re-reads all records and recomputes the full dis cache (same total work as hxFolio's async `updateAll`).
- After every `folio.sync(null, "dis")` call, the same sweep runs.
- On open/reopen, an initial sweep runs so that persisted disMacro dis values are available immediately.
- `enrichRefs(dict)` is called in both `doReadRecById` and `doReadByIds` to inject cached `disVal` into all Ref-typed tags in each returned dict, making `Dict.dis` (via `Etc.dictToDis` + `Macro.refToDis`) and `Ref.dis` correct for disMacro records.

### DEV-002 — Added to haxall/src/core/build.fan
**Reference:** The reference project lives as a separate parent workspace (not integrated into
the main haxall src/core/build.fan). This implementation adds `rustFolio/build.fan` to
`src/core/build.fan` so it's included in standard `./build.sh haxall` builds.

**Impact:** None on test results. Enables `rustFolio.pod` to be rebuilt automatically with
the rest of haxall.

## Production Phases (Post-Gate)

| Phase | Status | Description |
|-------|--------|-------------|
| P1 - History persistence | ✅ 2026-02-23 | redb HISTORY + HISTORY_META tables; HIS_READ/HIS_WRITE/HIS_STAT RPCs. Gate: 10200 verifies ALL GREEN. |
| P2 - Runtime integration | ✅ 2026-02-23 | folio.props backend selection in HxdBoot; hx init + hx run verified; API responding; 6 records persisted across restart. |
| P3 - Backup | ✅ 2026-02-23 | FolioBackup impl — consistent redb snapshot via logical table copy. BACKUP_CREATE (0x0050) RPC. Gate: ALL GREEN. |
| P4 - File storage | ✅ 2026-02-23 | FolioFile — Fantom-side disk delegation via LocalFolioFile. Gate: ALL GREEN. |
| R1 - Reconnect + Transient | ✅ 2026-02-23 | Reconnect-on-failure (kill+respawn, 3 attempts). Transient registry (Fantom-side, replayed on reconnect). Watch bug fixes (RustFolioRec shared instance + ticks fix). status(), isAlive(), Q3 phantom-commit detection. Gate: ALL GREEN. |
| P5 - Hardening | 🔄 in progress | O1 (incremental dis) ✅ bfe2f0b7. O2 (tag index) ✅ 6b997a4f. F1 (isSpec) ✅ (this commit). Prefix rename, benchmarking pending. |

### P1 Design — History Persistence

**Problem:** `RustFolioHis` stores all time-series data in a Fantom-side `AtomicRef` map.
History is lost on restart. The redb `HISTORY` table is scaffolded but unused.

**Approach:**
- Rust: HISTORY table key = `[u16 id_len][id bytes][8 bytes encoded_ticks]` (biased i64 for correct lexicographic ordering). Value = serialized Val bytes (write_val format). HISTORY_META table: per-point stat row `[u64 size][i64 first_ticks][i64 last_ticks]`.
- Rust server: implement `HIS_WRITE (0x0041)` and `HIS_READ (0x0040)` handlers. Add `HIS_STAT (0x0042)` for lightweight augmentHisTags queries.
- Fantom: `RustFolioConn` gets `hisWrite`, `hisRead`, `hisStat` methods. `RustFolioHis` becomes a thin Rust RPC wrapper. Stats cache (`Str:RustHisStat AtomicRef`) replaces in-memory items; used by `augmentHisTags`.
- Span semantics (1 item before span.start, up to 2 after span.end) handled Rust-side for efficiency.
- `FolioUtil.hisWriteCheck` still called Fantom-side for validation/normalization before sending to Rust.

### P2 Design — Runtime Integration

**Problem:** `HxdBoot.initFolio()` hardcodes `HxFolio.open(config)`. No mechanism to swap backend.

**Approach:**
- Modified `HxdBoot.initFolio()` to check `{dir}/folio.props` for a `backend` key.
- If `backend=rustFolio`, uses Fantom reflection (`Type.find("rustFolio::RustFolio")`) to call `RustFolio.open(config)`.
- Reflection avoids adding `rustFolio` as a compile-time dependency of `hxd`. Pod must be on classpath (it is when built with haxall).
- Default: `backend=hxFolio` (or absent `folio.props`) → `HxFolio.open` as before. Fully backward compatible.

**Usage:**
```bash
mkdir myproject
echo "backend=rustFolio" > myproject/folio.props
fan hx init -headless -suUser admin -suPass <pw> -httpPort 8081 myproject
fan hx run myproject
```

**Integration test results (2026-02-23):**
- `hx init`: Rust spawned, TCP connected, 6 records committed, Steady state ✓
- `hx run` (after kill+restart): `records=6 cur_ver=4` (redb persisted across restart) ✓
- Haystack v3 API: `GET /api/sys/about` → Haxall 4.0.5 ✓
- `GET /api/sys/read?filter=id` → all 6 records returned via API ✓

**Modified file:** `src/core/hxd/fan/HxdBoot.fan` (22 lines added to `initFolio()`)

### P3 Design — Backup

**Problem:** `RustFolio.backup()` throws `UnsupportedErr`. Backup is required for any production deployment — operators need a consistent, restorable snapshot of the database.

**Approach:** Logical backup via a new `BACKUP_CREATE (0x0050)` RPC. Rust opens a read transaction on the live database, iterates RECORDS + HISTORY + HISTORY_META tables, and writes all key-value pairs into a new redb database file at a caller-specified temp path. Fantom then zips the snapshot file and moves it to the backup directory.

**Why logical copy instead of file copy:**
- A raw `std::fs::copy` of the live `.redb` file risks copying a partially-written page during a concurrent commit.
- A logical copy (redb read transaction + new database write transaction) is guaranteed consistent — the read transaction pins the MVCC snapshot for the duration, and the destination database is a valid, self-contained redb file.
- Slightly slower for large databases (full table scan vs file copy), but correct. We can revisit if benchmarks show it's a bottleneck.

**Wire protocol:**
- `BACKUP_CREATE (0x0050)`:  Request: `[str dest_path]` (length-prefixed UTF-8). Response: empty on success, error frame on failure.

**Rust changes:**
- `protocol.rs`: Add `BACKUP_CREATE = 0x0050` opcode constant.
- `storage.rs`: Add `backup_create(dest_path: &Path) -> Result<()>`. Opens a read transaction, creates destination redb database, copies RECORDS / HISTORY / HISTORY_META tables (skipping any table that doesn't yet exist — empty database edge case).
- `server.rs`: Add `handle_backup_create` + wire into `dispatch()`.

**Fantom changes:**
- `RustFolioConn.fan`: Add `opBackupCreate = 0x0050`, `backupCreate(Str destPath)` method.
- `RustFolioBackup.fan` (new file): `const class RustFolioBackup : FolioBackup`. Backup dir = `{folio.dir}/../backup/`. `create()` runs in background (Fantom `Future` + `Actor`): checks `inProgress` flag, generates YYMMDD-hhmmss timestamp, calls `backupCreate` RPC with a temp path, zips snapshot into `{name}-{ts}.zip` at backup dir, deletes temp file, resolves future. `list()` scans dir for `*.zip` files and parses timestamps. `status()` / `summary()` / `monitor()` follow hxFolio's BackupMgr pattern.
- `RustFolio.fan`: Add `backupImpl` field, initialize in constructor, `backup()` returns it.

**Gate impact:** No new tests. Existing 10200 verifies must remain ALL GREEN after build.

### P4 Design — File Storage

**Problem:** `RustFolio.file()` throws `UnsupportedErr`. File storage is used by production runtimes to attach binary blobs (documents, images, exports) to records.

**Approach:** Delegate entirely to `LocalFolioFile`, which already exists in the `folio` pod. `LocalFolioFile` is a `@NoDoc` public class that stores blobs on the local filesystem in `{folio.dir}/../files/`, hashed into 1024 subdirectory buckets. It implements the full `FolioFile` interface including `get()` (spec validation via xeto), `delete()`, `withIn()`, `withOut()`, and async `fileSize` tag commits back to folio.

**Why no Rust involvement:** File blobs are large, variable-size binary data — the wrong workload for a record-oriented B-tree. The existing local filesystem implementation is the correct solution at this scale. A Rust blob table would add complexity with no benefit.

**Changes:**
- `RustFolio.fan`: Add `fileImpl` field of type `LocalFolioFile`, initialize in constructor, `file()` returns it.
- No new Rust code. No new Fantom files. No protocol changes.

**Gate impact:** No new tests. Existing verifies must remain ALL GREEN.

## Build Notes

### Running the Rust crate build
```bash
cd ~/.openclaw/workspace/fan-dev/haxall/src/core/rustFolio/rust
cargo build --release
# Binary output: target/release/rust-folio
# To install: cp target/release/rust-folio ../../../../bin/
```

### Building just the Fantom pod
```bash
cd ~/.openclaw/workspace/fan-dev/haxall
~/.openclaw/workspace/fan-dev/fan/bin/fan src/core/rustFolio/build.fan compile
```

### Running the test gate
```bash
cd ~/.openclaw/workspace/fan-dev/haxall
~/.openclaw/workspace/fan-dev/fan/bin/fant testFolio
```
