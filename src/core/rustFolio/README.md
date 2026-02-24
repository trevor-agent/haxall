# rustFolio

A Rust-backed implementation of the Haxall `Folio` storage engine.

## Overview

rustFolio replaces the default `hxFolio` (Java + flat-file) backend with a Rust
subprocess that uses [redb](https://github.com/cberner/redb) — a pure-Rust
embedded B-tree database — as its persistent store.

The architecture is a **two-process model**:

```
┌─────────────────────────────────────┐
│  Fantom JVM (Haxall runtime)        │
│                                     │
│  RustFolio (extends Folio)          │
│    │                                │
│    ├─ RustFolioProcess  ──spawn──►  │  ┌─────────────────────────────┐
│    ├─ RustFolioConn  ────TCP───►    │  │  rust-folio (native binary) │
│    ├─ RustFolioHis                  │  │                             │
│    ├─ RustFolioBackup (zip)         │  │  redb B-tree database       │
│    ├─ LocalFolioFile (disk)         │  │  Filter eval (native)       │
│    ├─ RustFolioDisMgr (dis cache)   │  │  History + Backup (redb)    │
│    ├─ recCache (FolioRec cache)     │  │                             │
│    └─ transientRegistry             │  └─────────────────────────────┘
│                                     │
│  RustFolioTestImpl ◄── testFolio ─  │
└─────────────────────────────────────┘
```

The Fantom pod manages lifecycle, watches, passwords, and display strings.
The Rust process owns the persistent record store, filter evaluation, and
history storage.

---

## Directory Structure

```
rustFolio/
├── README.md               ← this file
├── PROGRESS.md             ← milestone log and architectural deviations
├── build.fan               ← Fantom pod build script
├── fan/                    ← Fantom source (9 files)
│   ├── RustFolio.fan           main Folio subclass
│   ├── RustFolioBackup.fan     FolioBackup impl (redb snapshot + zip)
│   ├── RustFolioConn.fan       TCP binary protocol client
│   ├── RustFolioDisMgr.fan     disMacro evaluation + Ref.disVal cache
│   ├── RustFolioHis.fan        history implementation (backed by redb)
│   ├── RustFolioProcess.fan    Rust subprocess lifecycle
│   ├── RustFolioRec.fan        FolioRec wrapper over Dict
│   ├── RustFolioSerializer.fan binary encode / decode for all Haystack types
│   └── RustFolioTestImpl.fan   testFolio gate registration
└── rust/                   ← Rust crate (14 source files)
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs             CLI + tracing init
    │   ├── lib.rs              crate root
    │   ├── server.rs           TCP dispatch — ReadById/ReadAll/CommitAll/Sync
    │   ├── protocol.rs         binary framing (length-prefixed u8/u16/u32/u64/str)
    │   ├── types.rs            Haystack val types (Val, Dict, HRef, …)
    │   ├── types_ser.rs        serialize vals → wire; enrich_id_dis
    │   ├── types_de.rs         deserialize wire → vals
    │   ├── record_cache.rs     in-memory cache (persistent / transient / merged)
    │   ├── storage.rs          redb backend (RECORDS + HISTORY tables)
    │   ├── commit.rs           commit engine (add/update/remove, versioning)
    │   ├── query.rs            readAll / readCount over the cache
    │   ├── config.rs           FolioConfig deserialization
    │   ├── error.rs            FolioError (thiserror)
    │   └── filter/
    │       ├── mod.rs          filter entry point
    │       ├── ast.rs          filter AST nodes
    │       ├── parser.rs       hand-written recursive-descent parser
    │       └── eval.rs         filter evaluation against Dict
```

---

## How It Works

### Protocol

Communication is a custom **binary wire format** over a TCP loopback socket.

- The Rust process binds an ephemeral port, writes it to
  `{dir}/.rust-folio.port`, and prints `READY:{port}` to stdout.
- The Fantom process reads the port file and connects.
- Every request is: `[u32 total_len][u8 msg_type][u16 opcode][payload]`
- Every response mirrors the same frame structure.

Opcodes: `CLOSE`, `SYNC`, `CUR_VER`, `FLUSH_MODE`, `FLUSH`, `READ_BY_ID`,
`READ_BY_IDS`, `READ_ALL`, `READ_COUNT`, `COMMIT_ALL`, `HIS_READ`,
`HIS_WRITE`, `HIS_STAT`, `BACKUP_CREATE`.

### Ref.dis enrichment

hxFolio uses shared in-memory `Rec` objects with a single mutable `Ref.disVal`
per record — updating A's dis automatically propagates to any dict holding `@A`.

rustFolio has no shared Refs (each `readById` deserializes a fresh Dict). The
`RustFolioDisMgr` compensates:

1. After every commit and on `folio.sync(null, "dis")`, `updateAll()` reads all
   records from Rust, evaluates every `disMacro` pattern recursively (via
   `RustFolioMacro extends Macro`), and atomically swaps a `Str:Str` id→dis
   cache.
2. `enrichRefs(dict)` is called in `doReadRecById` and `doReadByIds` — it walks
   every Ref-typed tag in the returned dict and injects `Ref.disVal` from the
   cache. This makes `Dict.dis` (which calls `Etc.dictToDis` → `Macro.refToDis`
   → `Ref.dis`) resolve correctly for chained `disMacro` records.

---

## Building

### Prerequisites

- Fantom installed (tested with the fan-dev checkout)
- Rust stable toolchain (`rustup`)

### Build the Rust binary

```bash
cd rust/
cargo build --release
# binary: target/release/rust-folio
```

The compiled binary is located by `RustFolioProcess.fan` at runtime. It searches
(in order): `$FAN_HOME/bin/`, the dev cargo release path, the current directory,
and finally the system PATH.

### Build the Fantom pod

```bash
fan build.fan compile
```

Or build the whole Haxall tree (rustFolio is included in `src/core/build.fan`):

```bash
fan src/core/build.fan
```

### Run the test gate

```bash
fant testFolio
```

Expected: `All tests passed! [7 types, 23 methods, 10200 verifies]`

---

## Runtime Integration

rustFolio is wired into the Haxall runtime via `{dir}/folio.props`:

```bash
# Create project with rustFolio backend
mkdir myproject
echo "backend=rustFolio" > myproject/folio.props

# From inside the haxall/ dev directory:
fan hx init -headless -suUser admin -suPass <password> -httpPort 8081 myproject
fan hx run myproject
```

`HxdBoot.initFolio()` reads `folio.props` and uses Fantom reflection to call
`RustFolio.open(config)` — no compile-time dependency on rustFolio from hxd.
Omitting `folio.props` (or setting `backend=hxFolio`) falls back to the default.

---

## Known Limitations

### File storage uses local filesystem

`folio.file()` delegates to `LocalFolioFile` — the same on-disk bucket
implementation used by `hxFolio`. Files are stored under `{dir}/../files/`,
hashed across 1024 subdirectory buckets. This is correct for single-node
deployments. A cloud or distributed blob store would require a different
`FolioFile` implementation.

### Reconnect-on-failure

When the Rust process crashes, `RustFolio` detects the `IOErr` from the broken
TCP connection and reconnects automatically: kills the dead process, spawns a
fresh one, replays the transient registry to the new process, and recomputes
the dis cache. Up to 3 attempts, 1 second apart.

Write operations (commits) that were in-flight at crash time are **not** retried
— they propagate `IOErr` to the caller, who is responsible for recovery. Read
operations are retried once automatically.

A `WARN` log is emitted if `curVer` advanced during the crash window, indicating
a "phantom commit" (Scenario A: commit persisted but ACK never reached Fantom).

### Prefix rename unsupported

`PrefixTest` skips the `<Prefix id rename unsupported>` case.
hxFolio supports renaming all record ids when the project's id prefix changes.
This requires iterating all records and rewriting their ids atomically — a
non-trivial Rust-side operation that is not yet implemented.

---

## Design Notes

### TCP instead of Unix domain sockets

Fantom's `Socket` class is TCP-only — there is no Unix domain socket API.
rustFolio uses `127.0.0.1:0` (ephemeral port) with a port-file rendezvous
(`{dir}/.rust-folio.port`). Security is equivalent to a Unix socket since
the listener is loopback-only.

### History storage in redb

History items are persisted in redb's `HISTORY` table using a composite key:
`[u16 id_len][id_bytes][u64 biased_ticks]`. The tick bias (`XOR 0x8000...`)
maps signed i64 to u64 so that big-endian byte order yields natural chronological
ordering. Stats (size, first, last) are cached in `HISTORY_META` and updated
atomically on every write.

### Reconnect and transient registry

When the Rust process crashes, `doReadRecById` and `doCommitAllAsync` catch the
`IOErr` and call `doReconnect()`. Reconnect kills the dead process, spawns fresh,
reads the new `curVer` to check for phantom commits (Q3 detection), replays the
transient registry, recomputes the dis cache, and clears the his stats cache.

**Transient registry:** Transient commits (`Diff.transient`) are always updates
to existing persistent records (the Diff API rejects `transient+add`). After each
successful transient commit, `RustFolio` accumulates the effective transient tag
overlay per record in a Fantom-side `AtomicRef` map. On reconnect, each entry
is replayed as `Diff(rec, overlay, Diff.transient)` to the new Rust process.

**RustFolioRec cache:** `RustFolio.recCache` holds one canonical `RustFolioRec`
per record id. `doReadRecById` returns the cached instance (refreshing its dict
from Rust on each call). `ticks` is updated only on successful commit — not on
read — so watch polls only see a record as changed when it was actually committed.
`watchCount` persists on the shared instance across all reads.

### Backup via logical redb copy

`RustFolioBackup.create()` sends a `BACKUP_CREATE (0x0050)` RPC with a temp
file path. Rust opens a read transaction (pinning the MVCC snapshot), then
iterates RECORDS, META, HISTORY, and HISTORY_META tables and writes every
key-value pair into a new redb database at the temp path. Fantom zips the
snapshot into `{name}-YYMMDD-hhmmss.zip` in `{dir}/../backup/`, matching
hxFolio's path-prefix convention (`{name}-YYMMDD-hhmmss/db/db.redb`), then
deletes the temp file. The operation runs in a background actor and returns a
`FolioFuture`.

A logical copy (read transaction + new database write) is used instead of a
raw file copy because `std::fs::copy` on a live redb file risks partially-written
pages during a concurrent commit. The read transaction guarantees consistency.

### File storage via LocalFolioFile

`RustFolio.file()` returns a `LocalFolioFile` instance. Files are stored on the
local filesystem under `{dir}/../files/`, hashed into 1024 subdirectory buckets
(`b0/` through `b1023/`). `LocalFolioFile` handles the full `FolioFile` lifecycle
including spec validation (via the xeto namespace), `withIn`/`withOut` semantics,
and async `fileSize` tag commits back to folio. No Rust involvement — binary blobs
are the wrong workload for a record-oriented B-tree.

### Tag presence index for filter acceleration

`RecordCache` maintains a secondary index: `tag_name → Set<record_id>` tracking
which records currently have each tag in their merged (persistent + transient)
view. `query.rs` extracts leading `Has(single_tag)` terms from the top-level AND
chain of every incoming filter and intersects the corresponding index sets to
produce a candidate set before running the full filter evaluator. For a filter
like `point and his`, only records with both tags are visited — the rest are
skipped without evaluation. Filters with an `Or` at the root or no extractable
`Has` terms fall back to a full scan. The index is maintained incrementally on
every commit, including transient commits; the most common transient case (a value
update that changes no tag keys) has zero index cost.

### isSpec filter support

`isSpec("ph::Point")` checks whether a record's `spec` tag (a Ref to a Xeto type
name) is the named spec or any of its subtypes. For folio Dict records,
`MNamespace.specOf` simply reads `rec["spec"]` as a Ref id, so `isSpec` reduces
to a set membership check. At folio open and after each reconnect,
`RustFolio.syncSpec()` iterates all types in the Xeto namespace and builds a map
of `parent_qname → Set<all_subtype_qnames>`, which is pushed to the Rust process
via `SPEC_UPDATE (0x0060)`. The Rust evaluator resolves `isSpec` in O(1): two
hash lookups. The spec map is also populated on the first persistent commit after
the namespace becomes available — safe because `HxLibs.doUpdate()` sets the
namespace reference before calling `folio.commitAll()`.

### Fantom-side dis propagation

Ref.disVal is set by `RustFolioDisMgr` rather than by the Rust process because
disMacro evaluation requires resolving cross-record references — a query-level
operation that is simpler to implement in Fantom where the full record set is
already available post-read.

---

## Future Improvements

- **Prefix rename** — atomic id rewrite across all records in redb. `PrefixTest` currently skips this case.
- **Namespace reload hook** — `isSpec` filter evaluation uses a spec hierarchy pushed from Fantom at open/reconnect. Runtime lib changes require a restart to refresh the map. The correct fix is a `onNamespaceModified` callback contributed to `FolioHooks` upstream.
- **Incremental his_stat** — `HIS_WRITE` currently recomputes stats with a full range scan. Incremental maintenance using running min/max would eliminate this for large history sets.
- **Socket authentication** — a shared-secret handshake between Fantom and Rust would be appropriate for multi-tenant deployments where per-process isolation is not guaranteed.
- **Performance benchmarks** — no formal throughput or latency comparison against hxFolio has been conducted.

---

## Dependencies

**Rust crate** (`Cargo.toml`):

| Crate | Purpose |
|-------|---------|
| `redb` | Embedded B-tree database (persistent record + history storage) |
| `chrono` + `chrono-tz` | DateTime handling with full IANA timezone support |
| `clap` | CLI argument parsing |
| `thiserror` | Ergonomic error types |
| `tracing` + `tracing-subscriber` | Structured logging |

**Fantom pod** (`build.fan`):

`sys`, `concurrent`, `util`, `inet`, `xeto`, `haystack`, `folio`
