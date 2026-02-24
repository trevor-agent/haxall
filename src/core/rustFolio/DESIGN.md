# rustFolio — Architecture and Design

This document describes the architectural decisions behind rustFolio: why the system
is structured the way it is, what alternatives were considered, and where the
boundaries between Rust and Fantom were drawn. It is intended as a durable
reference for contributors and maintainers.

---

## 1. Motivation

Haxall's default storage engine, `hxFolio`, stores records as serialized flat files
managed by a Java-based store (`hxStore`). It is correct and well-tested, but it
carries several structural constraints:

- **Single-threaded commit path** with Java-level GC pressure on large record sets.
- **Flat-file layout** that makes point queries O(n) on disk without a separate index.
- **No embedded transactional log** — crash recovery depends on the file format's
  journaling, which was not designed for the write patterns of modern IoT runtimes.
- **History storage in flat files** — a separate append-only format that cannot be
  efficiently queried by span without a full scan.

rustFolio replaces the storage layer with [redb](https://github.com/cberner/redb) —
a pure-Rust embedded B-tree database with MVCC, durable commits, and a read API that
supports range queries natively. The goal is not to replace Haxall's Fantom runtime,
but to give it a better foundation beneath.

---

## 2. Architecture

### 2.1 Two-Process Model

rustFolio runs as a pair of processes:

```
┌──────────────────────────────────────────────────────────────────┐
│  Fantom JVM (Haxall runtime)                                     │
│                                                                  │
│  RustFolio (extends Folio)                                       │
│    ├─ RustFolioProcess    — subprocess lifecycle                  │
│    ├─ RustFolioConn       — binary TCP client                    │
│    ├─ RustFolioHis        — history (thin RPC wrapper)           │
│    ├─ RustFolioBackup     — FolioBackup (snapshot + zip)         │
│    ├─ LocalFolioFile      — FolioFile (local filesystem)         │
│    ├─ RustFolioDisMgr     — disMacro evaluation + dis cache      │
│    ├─ recCache            — canonical FolioRec per record id     │
│    └─ transientRegistry   — Fantom-side transient state          │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
         │  spawn
         ▼
┌──────────────────────────────┐
│  rust-folio (native binary)  │
│                              │
│  redb B-tree (RECORDS, META, │
│    HISTORY, HISTORY_META)    │
│  Filter eval (Rust-native)   │
│  Commit engine               │
│  History read/write/stat     │
│  Backup (logical table copy) │
└──────────────────────────────┘
```

The Rust process owns **persistence and query evaluation**. The Fantom process owns
**lifecycle, watches, passwords, hooks, display strings, and reconnect logic**.

### 2.2 Scope Boundary

The boundary was drawn based on a single principle: **put work where the data lives**.

| Concern | Owner | Reason |
|---------|-------|--------|
| Record persistence (CRUD) | Rust | Data lives in redb |
| Filter evaluation | Rust | Avoids full dataset serialization per query |
| History storage + read/stat | Rust | Data lives in redb HISTORY table |
| Backup (snapshot) | Rust + Fantom | Rust provides the snapshot; Fantom zips it |
| File storage (blobs) | Fantom (LocalFolioFile) | Large blobs don't belong in a B-tree |
| disMacro evaluation | Fantom | Requires cross-record ref resolution (query-level) |
| Pre/post-commit hooks | Fantom | Hook object identity required by Haxall framework |
| FolioWatch + ticks | Fantom | Watch lifecycle is a JVM/actor concern |
| Passwords | Fantom | Existing PasswordStore API |
| Transient record registry | Fantom | Transient state is ephemeral; Fantom owns recovery |

---

## 3. Wire Protocol

### 3.1 Transport

The Rust process binds an ephemeral TCP port on `127.0.0.1`. After binding, it
writes the port number to `{dir}/.rust-folio.port`. The Fantom process polls for
this file and connects when it appears.

**Why TCP instead of Unix domain sockets:** Fantom's `Socket` class is TCP-only.
There is no Unix domain socket API in the standard Fantom runtime. Loopback TCP
provides equivalent security properties (loopback-only, same machine) with no
meaningful latency difference for IPC on modern kernels.

**Why a port file instead of stdout:** Fantom's `Process` class routes subprocess
stdout to an `OutStream` (a writer, not a reader). Reading from the subprocess stdout
from the Fantom side requires a background thread and synchronization. Writing the
port to a file is simpler, atomic at the OS level, and safe to poll from any thread.

**Connection model:** The Rust server accepts exactly one TCP connection. This
simplifies the server significantly (no connection pool, no per-connection state)
and matches the single-writer-single-reader model of the Folio architecture.

### 3.2 Frame Format

Every message is a length-prefixed binary frame:

```
┌──────────────┬───────────┬───────────┬──────────────────┐
│ total_len    │ msg_type  │ opcode    │ payload           │
│ u32 (4 bytes)│ u8        │ u16 (2B)  │ (total_len - 3B) │
└──────────────┴───────────┴───────────┴──────────────────┘
```

`msg_type` values: `0x01` = Request, `0x02` = Response, `0x03` = Error.

Error frames carry a `u16` error code followed by a length-prefixed UTF-8 error
message. Error codes map to specific Fantom exception types (`CommitErr`,
`ConcurrentChangeErr`, `UnknownRecErr`, etc.) so that the Fantom client can throw
the semantically correct exception rather than a generic `IOErr`.

### 3.3 Handshake

On connect, the Fantom client sends a 6-byte handshake:
```
[4 bytes magic: 'R','F','O','L'] [2 bytes protocol version: 0x0001]
```
The server responds with 7 bytes (magic + version + status byte). A status of
`0x00` indicates acceptance; any other value indicates a version mismatch.

The handshake protects against connecting to a stale process left over from a
previous run or a different version of the binary.

### 3.4 Opcodes

| Range | Category |
|-------|----------|
| `0x0001–0x0003` | Lifecycle: OPEN, CLOSE, SYNC |
| `0x0010–0x0013` | Reads: READ_BY_ID, READ_BY_IDS, READ_ALL, READ_COUNT |
| `0x0020` | Writes: COMMIT_ALL |
| `0x0030–0x0032` | Metadata: CUR_VER, FLUSH_MODE, FLUSH |
| `0x0040–0x0042` | History: HIS_READ, HIS_WRITE, HIS_STAT |
| `0x0050` | Backup: BACKUP_CREATE |
| `0x0060` | Schema: SPEC_UPDATE |

### 3.5 Type Serialization

All Haystack values are serialized using a custom binary format defined in
`RustFolioSerializer.fan` (Fantom) and `types_ser.rs`/`types_de.rs` (Rust). The
format uses a 1-byte type tag followed by type-specific encoding. Key design choices:

- **Strings:** length-prefixed UTF-8 (`u32 len` + bytes).
- **DateTime:** `i64` ticks (nanoseconds since Fantom epoch 2000-01-01) + timezone name.
- **Ref:** length-prefixed id string + optional dis string (1-byte presence flag).
- **Number:** `f64` value + optional unit symbol.
- **Dict:** `u32` tag count + `(str key, val)` pairs.

The format is not a general-purpose serialization format — it is optimized for the
types that appear in Haystack record dictionaries.

---

## 4. Record Storage

### 4.1 redb Tables

The Rust process uses four redb tables:

| Table | Key Type | Value Type | Purpose |
|-------|----------|------------|---------|
| `records` | `&str` (Ref.id) | `&[u8]` (serialized Dict) | Persistent record store |
| `meta` | `&str` | `&[u8]` | System key/value pairs (`curVer`, etc.) |
| `history` | `&[u8]` (composite) | `&[u8]` (val bytes) | Time-series items |
| `history_meta` | `&str` (Ref.id) | `&[u8]` (stat row) | Per-point history statistics |

### 4.2 MVCC and Consistency

redb is an MVCC B-tree. Writers take a write transaction that is visible to
subsequent readers once committed. Read transactions see a consistent snapshot of
the database at the time they were opened.

The Rust server is single-threaded (one TCP connection, one request at a time), so
there is never concurrent write pressure within a single server instance. The MVCC
model is primarily leveraged for backup consistency.

### 4.3 Record Versioning

A monotonically increasing `curVer` counter is stored in the `meta` table and
incremented on every persistent `commitAll` call. Transient commits do not
increment `curVer`. The Fantom side caches the last known `curVer` for
phantom-commit detection after reconnect.

### 4.4 Commit Engine

The commit engine (`commit.rs`) processes each `Diff` in a batch:

1. **Persistent commits:** Serialize the new `Dict` to bytes, write to the
   `records` table, increment `curVer`, and commit the write transaction atomically.
   The record cache is updated in-place after the transaction commits.
2. **Transient commits:** Update only the in-memory `RecordCache.transient` overlay.
   redb is not involved. `curVer` does not advance.

Each `Record` in `RecordCache` maintains three layers:
- `persistent` — from redb; survives restart
- `transient` — in-memory overlay; lost on restart
- `merged` — `persistent + transient`; returned by reads

### 4.5 Ref Normalization and ID Prefixes

Projects in Haxall use a string prefix for all record ids (e.g., `p:myproject:r:`).
The Rust process handles prefix normalization in the commit engine: relative Refs
in tag values are expanded to absolute form, and the id tag is normalized before
persistence. `Ref.nullRef` (id `"null"`) is explicitly exempt.

---

## 5. Filter Evaluation

Filter evaluation runs natively in Rust. The Fantom client serializes the filter
as a string (`filter.toStr`), sends it in the `READ_ALL` or `READ_COUNT` payload,
and Rust evaluates it against the in-memory `RecordCache`.

**Why Rust-side evaluation:** The alternative — serializing all records to Fantom
and filtering there — would transfer O(n) data per query. For large record sets
(thousands of records), this would be prohibitively expensive. Running the filter
against the in-memory cache in Rust is O(n) CPU but zero network transfer for
non-matching records.

The filter parser (`filter/parser.rs`) is a hand-written recursive-descent parser
that handles the full Haystack filter grammar. The evaluator (`filter/eval.rs`)
walks the AST against each `Record.merged` dict.

### 5.1 Tag Presence Index

`RecordCache` maintains a secondary index: `tag_index: HashMap<String, HashSet<String>>`
mapping tag name → set of record ids that currently have that tag in their
**merged** view (persistent + transient). This allows common filter patterns like
`point and his` or `site and geoCity` to skip the full record scan entirely.

**Query acceleration:** `query.rs` walks the top-level AND chain of every incoming
filter looking for simple `Has(single_tag)` terms. If any are found, their index
sets are intersected to produce a candidate set. The full filter is then evaluated
only against candidates. A filter with no extractable `Has` terms (e.g., a
comparison filter, or an `Or` at the root) falls back to a full scan — the correct
conservative choice.

**Index maintenance:** The index is updated on every `load`, `apply_persistent_commit`,
and `apply_transient_commit`. For transient commits (e.g., `curVal` updates), the
update uses a snapshot-diff of the merged tag-key set before and after the commit.
In the common case where a transient commit only changes a value without adding or
removing a tag key, the diff is empty and the index maintenance cost is zero.

### 5.2 isSpec Filter Support

`isSpec("ph::Point")` checks whether a record's `spec` tag (a Ref to a Xeto type
name) is the named spec or any of its subtypes. For Dict records, `MNamespace.specOf`
simply reads `rec["spec"]` as a Ref id — so `isSpec` reduces to a set membership
check: is the record's spec Ref id in the set of all types that are-a `ph::Point`?

**Spec hierarchy push (SPEC_UPDATE, 0x0060):** At folio open and after each
reconnect, `RustFolio.syncSpec()` iterates all types in the Xeto namespace and
builds a map: parent spec qname → set of all sub-spec qnames (including the parent
itself). This map is sent to the Rust process via `SPEC_UPDATE`. The Rust evaluator
then resolves `IsSpec(name)` in O(1): look up the record's `spec` Ref id in the
`spec_subtypes[name]` set.

**Lazy initialization:** The Xeto namespace is typically unavailable at folio
open time (libs are loaded after the folio is opened). `syncSpec()` sends an empty
map if the namespace is null. A lazy check in `doReadAll` and `doReadCount` triggers
a one-time re-sync the first time either is called after the namespace becomes
available.

**Runtime lib changes:** If Xeto libs are added or removed at runtime, the Rust-side
spec map goes stale until the next restart or reconnect. There is no public
`FolioHooks` callback for namespace reload. The follow-up integration task is to
contribute `onNamespaceModified(Namespace)` to `FolioHooks` upstream.

**Why not filter rewriting (Option C):** The alternative — rewriting `isSpec` nodes
to an `Or` of `Eq` comparisons before sending the filter to Rust — was rejected
because `ph::Point` has 100–200 subtypes in a production ph library. An `Or` of
200 `Eq` nodes would be slower than the current full scan and would bloat the
filter string significantly.

---

## 6. History Storage

### 6.1 Table Key Design

History items are stored in the `history` table with a composite key:

```
[2 bytes: id_len as u16 big-endian]
[id_len bytes: point id UTF-8]
[8 bytes: ticks XOR 0x8000_0000_0000_0000, big-endian]
```

The XOR bias maps signed `i64` ticks to `u64` such that the natural big-endian
byte order yields correct chronological ordering (`i64::MIN` sorts first). This
allows efficient range scans with B-tree seeks — no full table scan is needed for
span queries.

### 6.2 Span Semantics

`HIS_READ` supports two modes:

- **All:** Returns every item for the point.
- **Span:** Returns items in a half-open interval `[start, end)` with SkySpark
  boundary semantics: up to one item before `span.start` (for interpolation) and
  up to two items at or after `span.end` (for post-span context).

Boundary items are retrieved using B-tree range seeks before and after the primary
span. This matches what hxFolio's history reader provides to clients.

### 6.3 Statistics Cache

`HIS_STAT` returns `(size, first_ticks, last_ticks)` for a point without reading
any history items. This row is stored in `history_meta` and updated atomically on
every `HIS_WRITE`.

The Fantom side maintains a `Str:RustHisStat` stats cache (in `RustFolioHis`) so
that `augmentHisTags` (which injects `hisSize`, `hisStart`, `hisEnd` into record
dicts) can service repeated reads without an RPC per record. The cache is cleared
on reconnect and lazily re-populated.

### 6.4 Validation

History write validation (`FolioUtil.hisWriteCheck`, `hisWriteMerge`) runs
Fantom-side before sending to Rust. This preserves the existing Haxall validation
semantics (timezone consistency, unit matching, timestamp precision) without
duplicating them in Rust.

---

## 7. Display String Management

### 7.1 Why Fantom-Side

Haystack's `disMacro` pattern evaluates expressions like `"{equip} / {point}"` by
resolving Ref-valued tags recursively across records. This requires:

1. A full-record read for each ref in the chain.
2. Recursive pattern evaluation.
3. Knowledge of the Xeto namespace for spec-based display.

All of this is simpler to implement in Fantom where the full record set is already
accessible post-read and the Xeto API is native. Implementing disMacro evaluation
in Rust would require reimplementing significant parts of the Haxall type system.

### 7.2 Cache Design

`RustFolioDisMgr` maintains a `Str:Str` id→computed-dis map in an `AtomicRef<Unsafe<Map>>`. After every persistent commit and on `folio.sync(null, "dis")`, `updateAll()` reads all records from Rust, evaluates every `disMacro` pattern recursively via `RustFolioMacro extends Macro`, and atomically swaps the cache map.

`enrichRefs(dict)` is called in `doReadRecById` and `doReadByIds` to inject
`Ref.disVal` into every Ref-typed tag in returned dicts. This ensures that
`Dict.dis` (which calls `Etc.dictToDis` → `Macro.refToDis` → `Ref.dis`) resolves
correctly for chained `disMacro` records, matching hxFolio's behavior.

**Known cost:** `updateAll()` is O(n) per commit. For large databases this is the
primary performance concern. Mitigation (dirty-set tracking) is planned but not yet
implemented.

---

## 8. FolioRec Cache

### 8.1 The Problem

`FolioRec` is the interface used by Haxall's watch system (`HxWatch`) to track
per-record state:

- `ticks()` — nanoseconds of last change. `HxWatch.poll(lastPoll)` returns only
  records whose `ticks > lastPoll`. If `ticks` equals "now" on every read, every
  record appears modified on every poll.
- `watchCount()` / `watchIncrement()` / `watchDecrement()` — track how many active
  watchers are subscribed to a record. Used to determine when to fire first-watch
  and last-watch events.

hxFolio maintains one shared `Rec` object per record id in an in-memory index.
`ticks` is updated only on commit; `watchCount` persists across all reads.

If rustFolio creates a new `RustFolioRec` on every `doReadRecById` call, both
contracts break: `ticks` is always "now", and `watchCount` resets to 0 every time.

### 8.2 The Fix: Canonical Instance Cache

`RustFolio.recCache` is a `ConcurrentMap<Str, RustFolioRec>` that holds one
canonical `RustFolioRec` per record id.

`doReadRecById`:
- Fetches the current dict from Rust.
- Looks up the existing `RustFolioRec` in `recCache`; creates one if absent.
- Calls `rec.refreshDict(dict)` to update the dict content **without** changing
  `ticks` or `watchCount`.
- Returns the cached instance.

After a successful commit:
- For each committed record, the cached instance is stamped via `updateOnCommit(dict)`,
  which sets `ticks = Duration.nowTicks`. Watch polls after a commit will see
  affected records as changed — and only those records.

`RustFolioRec.ticks` starts at `1` (not "now") so that a record that has never
been committed through this Fantom session is invisible to watch polls until it
actually changes.

The cache also provides resilience during reconnect: if the Rust process is
temporarily unavailable and `conn == null`, `doReadRecById` returns the stale cached
instance rather than throwing `ShutdownErr`. Watch polls continue to function during
the brief reconnect window.

---

## 9. Backup

### 9.1 BACKUP_CREATE RPC

When `RustFolioBackup.create()` is called, it sends `BACKUP_CREATE (0x0050)` with
an absolute path to a temporary file. The Rust handler:

1. Opens a read transaction on the live database (pinning the MVCC snapshot).
2. Creates a new redb database at the destination path.
3. Iterates `RECORDS`, `META`, `HISTORY`, and `HISTORY_META` tables, copying every
   key-value pair. Tables that don't yet exist (empty database) are silently skipped.
4. Commits the destination write transaction.
5. Releases the read transaction.

### 9.2 Why Logical Copy Instead of File Copy

A raw `std::fs::copy` on the live `.redb` file is not safe during concurrent writes.
redb's MVCC guarantees that pages referenced by open read transactions will not be
recycled, but a file copy reads bytes from disk — if a write transaction is committing
concurrently, the file may contain a mix of old and new page versions.

The logical copy (read transaction + new database write) is guaranteed consistent
because the read transaction pins the MVCC snapshot for the duration. The resulting
backup file is a valid, self-contained redb database that can be opened and
inspected independently.

### 9.3 Zip Format

The Fantom side wraps the snapshot in a zip file:

```
{name}-YYMMDD-hhmmss.zip
  └─ {name}-YYMMDD-hhmmss/
       └─ db/
            └─ db.redb
```

This path-prefix convention mirrors hxFolio's backup format, making the backup
compatible with tooling that expects the standard structure. The backup directory
is `{folio.dir}/../backup/`. The temporary snapshot file is deleted after zipping.

---

## 10. File Storage

`RustFolio.file()` returns a `LocalFolioFile` instance — a class already provided
by the `folio` pod that stores blobs on the local filesystem.

**Why not a Rust blob table:** Binary blobs (documents, images, exports) are
large, variable-size, and accessed sequentially. These are not the characteristics
that a sorted B-tree optimizes for. A Rust blob table would add implementation
complexity with no meaningful performance benefit over direct filesystem I/O.

`LocalFolioFile` stores files at `{folio.dir}/../files/`, hashed into 1024
subdirectory buckets (`b0/` through `b1023/`) to avoid directory entry limits.
It handles the full `FolioFile` lifecycle: spec validation (via the Xeto namespace),
`withIn`/`withOut` semantics, and async `fileSize` tag commits back to folio.

---

## 11. Transient Records

### 11.1 Constraint

The Haxall `Diff` API does not permit `TRANSIENT + ADD` or `TRANSIENT + REMOVE`
as combined flags — both combinations throw `DiffErr`. Transient commits are
always **updates to existing persistent records**: they add an in-memory tag
overlay (e.g., `curVal`, `curStatus`) that sits on top of the persistent base.

This is an important design constraint: **there are no transient-only records**.
Every record that has a transient overlay also has a persistent base in redb.

### 11.2 Fantom-Side Transient Registry

Because the Rust process holds transient state only in-memory (`RecordCache.transient`),
a Rust process crash loses all transient overlays. The persistent base records
survive (redb durability), but the runtime state (current values, status tags) is gone.

`RustFolio` maintains a `transientRegistry` — a `Str:TransientEntry` map stored in
an `AtomicRef` — that tracks the current effective transient tag overlay for each
record. After every successful transient commit, the registry is updated by merging
the diff changes into the existing entry for that record (with `None.val` markers
removing keys from the overlay).

This makes Fantom the source of truth for transient state; the Rust process is a
cache and evaluation engine.

### 11.3 Replay on Reconnect

After a successful reconnect, `replayTransients()` iterates the registry and sends
one `Diff(rec, overlay, Diff.transient)` per entry to the new Rust process. Records
whose persistent base has disappeared (an edge case; requires a concurrent persistent
remove during the crash window) are silently skipped and cleaned from the registry.

Transient replay precedes the dis cache recompute. This ensures that transient
records are visible to the dis computation (since dis patterns may reference
transient tags).

---

## 12. Reconnect-on-Failure

### 12.1 Failure Taxonomy

Three physical scenarios produce the same observable symptom — `IOErr` thrown by
`RustFolioConn` during a request:

| Scenario | Rust state at crash | redb state |
|----------|---------------------|------------|
| A | Crashed after committing, before sending ACK | Commit persisted |
| B | Crashed before processing the request | Commit not persisted |
| C | Crashed before receiving the request | Commit not persisted |

Scenarios B and C are clean failures: the caller receives `IOErr`, nothing changed.
Scenario A is the dangerous case: the caller receives `IOErr` but the data is
durably in redb.

### 12.2 Phantom Commit Detection (Q3)

After reconnect, `doReconnect()` reads `curVer` from the new process and compares
it against `lastKnownVerRef` — the last `curVer` that Fantom successfully confirmed
before the crash. If the new `curVer` is higher, Scenario A occurred:

```
log.warn("rust-folio reconnect detected phantom commit (curVer N → M)")
```

This is a diagnostic signal, not an automatic recovery. The data is in redb; the
caller who received the `IOErr` must decide whether to retry, verify, or escalate.

### 12.3 Reconnect Strategy

Write operations (`doCommitAllAsync`) are **not** retried after reconnect. Retrying
a write without knowing whether it already persisted risks double-committing.

Read operations (`doReadRecById`, `doReadAll`, `doReadCount`, etc.) are retried
once automatically after reconnect — they are idempotent.

The reconnect algorithm:

1. Close old connection (best-effort).
2. Kill old process via `RustFolioProcess.kill()` if `isAlive()`.
3. Spawn a fresh process and connect.
4. Read `curVer` for phantom commit detection.
5. Install new connection and process into `connRef`/`processRef` atomically.
6. Replay transient registry.
7. Recompute dis cache.
8. Clear his stats cache (lazy re-population).

Up to 3 attempts are made, with a 1-second pause between each. If all fail, `connRef`
is set to `null` and `ShutdownErr` is thrown. The `status()` method returns
`"reconnecting"` during this window and `"error"` afterward.

Reconnect runs synchronously within the folio actor (which is single-threaded), so
all pending operations queue naturally — no locking or state machine is required.

### 12.4 Watch Resilience During Reconnect

During the reconnect window, `conn == null`. The Haxall watch system calls
`folio.readRecById(id, false)` on each poll interval. Rather than throwing
`ShutdownErr`, `doReadRecById` returns the stale `RustFolioRec` from `recCache`.
Watches continue to function with the last-known data.

After reconnect and transient replay, `updateOnCommit` stamps all replayed records
with the current `Duration.nowTicks`. The next watch poll sees these records as
changed — correct behavior, since from the subscriber's perspective they were
temporarily absent and are now restored.

---

## 13. Runtime Integration

### 13.1 Backend Selection

Selecting the rustFolio backend requires a single file in the project directory:

```
{project-dir}/folio.props
  backend=rustFolio
```

`HxdBoot.initFolio()` reads this file and uses Fantom reflection to call
`RustFolio.open(config)`. This avoids adding a compile-time dependency on the
`rustFolio` pod from `hxd` — the pod is resolved at runtime if and only if the
backend is configured.

Omitting `folio.props` or setting `backend=hxFolio` falls back to the default
behavior. The change is fully backward compatible.

### 13.2 Build Integration

`rustFolio` is included in `src/core/build.fan` alongside the other core pods.
This means `./build.sh haxall` rebuilds the rustFolio Fantom pod automatically.
The Rust binary must be built separately with `cargo build --release` and placed
in `$FAN_HOME/bin/rust-folio` for production use.

At runtime, `RustFolioProcess` searches for the binary in this order:
1. `$FAN_HOME/bin/rust-folio` (production)
2. `../rustFolio/rust/target/release/rust-folio` (dev, relative to `haxall/`)
3. Current directory
4. System `PATH`

---

## 14. Security Model

The Rust process binds exclusively to `127.0.0.1` (loopback). No external network
access is possible. The connection is established immediately after process spawn;
the listening port is closed after the first connection is accepted.

Authentication between the Fantom and Rust processes is not implemented. The threat
model assumes that any process running on the same machine with the ability to
connect to a loopback port is already within the trust boundary of the Haxall
deployment. A shared-secret handshake would be appropriate for multi-tenant
environments where process isolation is not guaranteed.

Password storage for Haxall user accounts remains in `passwords.props` on the
Fantom side — the Rust process has no knowledge of authentication.

---

## 15. Decision Log

The following table summarizes the major design decisions and their rationale.
Decisions are identified by the codes used in `PROGRESS.md`.

| ID | Decision | Rationale |
|----|----------|-----------|
| DEV-001 | TCP over Unix domain sockets | Fantom's `Socket` class is TCP-only; no UDS API exists in the standard runtime. Security properties are equivalent on loopback. |
| DEV-002 | Port-file signaling over stdout | Fantom's `Process.in/out` semantics are inverted from what you might expect; reading from child stdout from Fantom requires a background thread. A port file is atomic, thread-safe, and simple. |
| DEV-003 | Single-connection server | Matches the single-writer model of Folio. Eliminates per-connection state and concurrency concerns in the Rust server. |
| DEV-004 | Hook dispatch Fantom-side | Pre/post commit hook objects have Fantom identity (the hook is a Fantom mixin instance). Dispatching from Rust would require serializing hooks, which is not feasible. |
| DEV-005 | disMacro evaluation Fantom-side | Requires cross-record ref resolution — a query-level operation that is simpler in Fantom where the full record set is available post-read. |
| DEV-006 | Logical backup over file copy | Raw file copy during active writes risks partially-written pages. A read transaction pins the MVCC snapshot, guaranteeing consistency at the cost of a full table scan. |
| DEV-007 | File blobs via LocalFolioFile | Large binary blobs are a poor fit for a sorted B-tree. LocalFolioFile (already in the folio pod) provides correct semantics with no new implementation. |
| DEV-008 | Transient registry Fantom-side | Rust's transient layer is in-memory and lost on crash. Keeping the authoritative copy in Fantom allows replay without protocol changes or Rust-side WAL. |
| DEV-009 | Rec cache for watch correctness | Without a canonical per-record FolioRec instance, `ticks` is always "now" (every record appears modified on every poll) and `watchCount` is always 0 (subscriptions don't persist). The rec cache mirrors hxFolio's shared Rec design. |
| DEV-010 | Let IOErr propagate on writes | Retrying a write without knowing whether it already persisted risks double-committing. The phantom commit warning (Q3) gives operators the information they need to recover manually. |
| DEV-011 | History key: biased ticks | XOR-biasing signed i64 ticks to u64 maps negative timestamps to the low end of u64, giving correct chronological ordering in redb's big-endian B-tree without any additional index. |
| DEV-012 | Reconnect inline in folio actor | The folio actor is single-threaded; reconnect within an actor message causes subsequent messages to queue naturally. No explicit state machine, no locking, no reconnect thread. |
| DEV-013 | Tag presence index in RecordCache | Secondary `HashMap<tag, HashSet<id>>` maintained against the merged view. Enables O(result_set) evaluation for Has-based filter leading terms by intersecting candidate sets before the full filter runs. Falls back to full scan for Or-rooted filters and non-Has leading terms. |
| DEV-014 | isSpec via pushed spec hierarchy (SPEC_UPDATE) | Rust resolves isSpec in O(1) using a parent→subtypes map pushed from Fantom at open/reconnect. Rejected filter rewriting (Option C): ph::Point has 100–200 subtypes in production; an Or of 200 Eq nodes is worse than a full scan. |

---

## 16. Future Work

The following improvements are planned but not yet implemented:

**Incremental history statistics (O3):** `HIS_STAT` currently returns a pre-computed
stat row. `HIS_WRITE` updates the stat row in-place (a single metadata write per
write batch). Incremental stat maintenance (running min/max) can eliminate the
occasional need for a full history scan on stat correction.

**Prefix rename:** Haxall supports renaming the project's record id prefix. This
requires rewriting all record ids and all Ref-valued tags atomically — a non-trivial
Rust-side operation that is not yet implemented. The `PrefixTest` gate case for
prefix rename is currently skipped.

**Socket authentication:** A shared-secret handshake in the protocol would be
appropriate for deployments where per-process isolation is not guaranteed.

**Namespace reload hook:** When Xeto libs are added or removed at runtime, the
Rust-side spec subtype map (used for `isSpec` filter evaluation) goes stale until
the next restart or reconnect. The correct fix is a `onNamespaceModified(Namespace)`
callback contributed to `FolioHooks` upstream, allowing all Folio implementations
to react to namespace changes. Until that hook exists, restart is the recovery path.

**Performance benchmarks:** No formal throughput or latency comparison against
hxFolio has been conducted. Baseline benchmarks at representative record counts
(1k, 10k, 100k records) with mixed read/write workloads would validate the
design assumptions.
