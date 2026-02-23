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
│    ├─ RustFolioHis (in-memory)      │  │                             │
│    └─ RustFolioDisMgr (cache)       │  │  redb B-tree database       │
│                                     │  │  Filter eval (native)       │
│  RustFolioTestImpl ◄── testFolio ─  │  │  History table (scaffolded) │
└─────────────────────────────────────┘  └─────────────────────────────┘
```

The Fantom pod manages lifecycle, watches, passwords, history, and display
strings. The Rust process owns the persistent record store and filter
evaluation.

---

## Status

**All testFolio milestones complete. Gate: 10,195 verifies — ALL GREEN.**

| Milestone | Status | Verifies |
|-----------|--------|----------|
| M0 — Scaffold | ✅ | — |
| M1 — CRUD + protocol | ✅ | |
| M2 — Filters + readAll | ✅ | 9,921 |
| M3 — Transient commits | ✅ | 9,921 |
| M4 — Hooks (preCommit / postCommit) | ✅ | 9,921 |
| M5 — History (HisTest) | ✅ | 10,164 |
| M6 — Display strings (DisTest) | ✅ | 10,195 |
| M7 — No deferred no-ops | ✅ | 10,195 |

See `PROGRESS.md` for full milestone history and deviation notes.

---

## Directory Structure

```
rustFolio/
├── README.md               ← this file
├── PROGRESS.md             ← milestone log and architectural deviations
├── build.fan               ← Fantom pod build script
├── fan/                    ← Fantom source (8 files)
│   ├── RustFolio.fan           main Folio subclass
│   ├── RustFolioConn.fan       TCP binary protocol client
│   ├── RustFolioDisMgr.fan     disMacro evaluation + Ref.disVal cache
│   ├── RustFolioHis.fan        in-memory history (HisTest-complete)
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
- Every request is: `[u8 msg_type][u16 opcode][u32 payload_len][payload]`
- Every response is: `[u8 msg_type][u16 opcode][u32 payload_len][payload]` or
  an error frame.

Opcodes: `CLOSE`, `SYNC`, `CUR_VER`, `FLUSH_MODE`, `FLUSH`, `READ_BY_ID`,
`READ_BY_IDS`, `READ_ALL`, `READ_COUNT`, `COMMIT_ALL`.

### Ref.dis enrichment (M6)

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

The compiled binary is embedded/located by `RustFolioProcess.fan` at runtime.

### Build the Fantom pod

```bash
fan build.fan compile
```

Or build the whole Haxall tree (rustFolio is in `src/core/build.fan`):

```bash
fan src/core/build.fan
```

### Run the test gate

```bash
fant testFolio
```

Expected: `All tests passed! [7 types, 23 methods, 10195 verifies]`

---

## Known Limitations (Pre-Production)

### Not yet integrated with the Haxall runtime (`hx init` / `hx run`)

rustFolio passes the testFolio gate but is **not yet wired as a selectable
folio backend** in the Haxall runtime. hxFolio remains the default. Wiring
rustFolio into the runtime requires:

- A service registration index entry (not just `testFolio.impl`)
- An `HxFolioFactory` equivalent or runtime configuration hook
- Validation under live runtime conditions (connectors, Axon eval, UI reads)

### History is in-memory only

`RustFolioHis` stores all time-series data in a Fantom-side `AtomicRef` map.
**History is lost on restart.** The redb `HISTORY` table is scaffolded and the
binary protocol supports history ops, but the Rust-side persistence path is not
yet implemented.

### Backup and file storage unsupported

`folio.backup()` and `folio.file()` throw `UnsupportedErr`. Neither is
exercised by the testFolio gate but both are exercised by production runtimes.

### Single-connection model

The Rust server accepts exactly one TCP connection. Reconnection after an
unexpected disconnect is not handled. Production use would require a
connect-with-retry strategy in `RustFolioConn`.

### Prefix rename unsupported

`PrefixTest` skips the `<Prefix id rename unsupported>` case.
hxFolio supports renaming all record ids when the project's id prefix changes.
This requires iterating all records and rewriting their ids atomically — a
non-trivial Rust-side operation not yet implemented.

### Full-sweep dis update after every commit

`RustFolioDisMgr.updateAll()` reads all records from Rust after every commit.
For databases with thousands of records this is O(n) per commit. hxFolio does
the same work but lazily (async actor, coalesced updates). Optimization
opportunities: dirty-set tracking, incremental propagation, or a Rust-side
DisMgr that sets `dis` on records at commit time.

---

## Roadmap to Production

See `PROGRESS.md` for milestone history. Remaining work, roughly prioritised:

| Phase | Work |
|-------|------|
| **P1 — History persistence** | Move history write/read to Rust (redb HISTORY table already scaffolded). Retire in-memory `RustFolioHis`. |
| **P2 — Runtime integration** | Service index registration. `hx init` flag / project config to select rustFolio. Validation under live runtime. |
| **P3 — Backup** | Implement `FolioBackup` — at minimum a consistent snapshot of the redb file. |
| **P4 — File storage** | Implement `FolioFile` — either Rust-side blob table or Fantom-side delegation to disk. |
| **P5 — Perf + hardening** | Benchmarking vs hxFolio. Incremental dis updates. Reconnect-on-failure. Prefix rename support. Concurrent-client support if needed. |

---

## Design Deviations from hxFolio

See `PROGRESS.md` §Deviations for the full list. Key ones:

- **DEV-003:** Fantom `Socket` is TCP-only — no Unix domain sockets. We use
  `127.0.0.1:0` (ephemeral port) with a port-file rendezvous.
- **DEV-006:** History stored Fantom-side (in-memory) rather than in redb.
  Allows M5 to pass the gate without Rust-side history ops.
- **DEV-007:** dis propagation uses a Fantom-side id→dis cache + `enrichRefs`
  rather than shared in-memory Ref objects (which don't exist in the decoupled
  process model).

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
