# rustFolio Progress

## Build Environment

| | |
|-|-|
| **Workspace** | `~/.openclaw/workspace/fan-dev/` |
| **Branch** | `rust-folio` (fork of `haxall/haxall`, branched from `main` at 4.0.5) |
| **Remote** | `trevor-agent` → `https://github.com/trevor-agent/haxall` |
| **Rust** | 1.83.0 |
| **Fantom/Haxall** | 4.0.5 |
| **Build** | `fan src/core/rustFolio/build.fan compile` |
| **Gate** | `fant testFolio` |

---

## Milestone History

All milestones and production phases are complete. Recorded here for reference.

| Milestone | Date | Gate |
|-----------|------|------|
| M0 Scaffolding | 2026-02-23 | 7477 verifies ✅ |
| M1 Basic CRUD | 2026-02-23 | green ✅ |
| M2 Filters | 2026-02-23 | 9921 verifies ✅ |
| M3 Transient | 2026-02-23 | 9906 verifies ✅ |
| M4 Hooks | 2026-02-23 | (with M1/M2) ✅ |
| M5 History | 2026-02-23 | 10164 verifies ✅ |
| M6 Display | 2026-02-23 | 10195 verifies ✅ |
| M7 Full Green | 2026-02-23 | 10195 verifies, 0 failures ✅ |
| P1 History persistence | 2026-02-23 | 10200 verifies ✅ |
| P2 Runtime integration | 2026-02-23 | hx init + hx run verified ✅ |
| P3 Backup | 2026-02-23 | green ✅ |
| P4 File storage | 2026-02-23 | green ✅ |
| R1 Reconnect + Transient | 2026-02-23 | green ✅ |
| O1 Incremental dis updates | 2026-02-23 | `bfe2f0b7` ✅ |
| O2 Tag presence index | 2026-02-23 | `6b997a4f` ✅ |
| F1 isSpec filter support | 2026-02-23 | `f964320a` ✅ |
| N2 Prefix rename | 2026-02-23 | `a03d168e` ✅ |
| L1 Log format alignment | 2026-02-23 | `dd589dd6` ✅ |
| O3 Incremental his_stat | 2026-02-23 | `70220ac5` ✅ |
| N1 Chunked readAll | 2026-02-23 | `7139e1fa` ✅ |

---

## Open Items

Ordered by implementation priority. All are tracked in DESIGN.md §16 Future Work.

### 1. S1 — Socket Authentication

**Priority:** Medium (security)
**Effort:** Small-Medium
**Scope:** Protocol change — both Rust and Fantom

The current handshake (`HELLO` opcode exchange) has no authentication. Any process
that can connect to the loopback port in the window between `READY:{port}` and the
Fantom connection gets access to the database.

Approach: shared-secret challenge-response. At startup, Rust generates a random
token and writes it to a temp file (or passes it via environment variable). Fantom
reads the token and includes it in the `HELLO` payload. Rust verifies the token,
rejects unknown clients, and deletes the token file. Requires no crypto beyond a
constant-time string comparison.

---

### 2. Namespace Reload Hook (Upstream Contribution)

**Priority:** Low (upstream Haxall)
**Effort:** Small
**Scope:** `folio::FolioHooks` + `hxm::HxFolioHooks` — upstream contribution

`isSpec` filter evaluation uses a spec hierarchy pushed from Fantom at open/reconnect
(`SPEC_UPDATE`, 0x0060). If Xeto libs are added or removed at runtime, the Rust-side
map goes stale until the next restart or reconnect.

The correct fix: add `onNamespaceModified(Namespace ns): Void` to `FolioHooks`.
`HxFolioHooks` overrides it and calls `rt.onNamespaceModified` (already exists) plus
notifies the Folio implementation. `RustFolio` overrides it to call `syncSpec()`.

This requires a PR to the upstream `haxall/haxall` repo. Design is straightforward;
the main work is the upstream coordination.

---

### 3. Performance Benchmarks

**Priority:** Low (validation)
**Effort:** Medium
**Scope:** Tooling — no production code changes

No formal throughput or latency comparison against hxFolio has been conducted.
Target: benchmark at 1K, 10K, and 100K records with mixed read/write/filter workloads.
Expected advantage for rustFolio: read-heavy workloads with indexed Has-filters.
Expected parity: write throughput (both bottlenecked by the single-writer redb model
vs. hxFolio's actor queue).

Defines the "done" criteria for the optimization work above.

---

## Notes

- Design rationale for all items above is in `DESIGN.md §16 Future Work`.
- Architectural decisions are in `DESIGN.md §15 Decision Log` (DEV-001 through DEV-014).
- Use `DESIGN.md` and `README.md` for permanent, upstream-mergeable content.
  PROGRESS.md is a working tracker and is intentionally not upstream-mergeable.
