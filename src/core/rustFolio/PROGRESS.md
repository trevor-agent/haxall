# rustFolio Progress

## Build Environment

| | |
|-|-|
| **Workspace** | `~/.openclaw/workspace/fan-dev/` |
| **Branch** | `rust-folio` (fork of `haxall/haxall`, branched from `main` at 4.0.5) |
| **Remote** | `trevor-agent` → `https://github.com/trevor-agent/haxall` |
| **Rust** | 1.83.0 |
| **Fantom/Haxall** | 4.0.5 |
| **Build (Fantom pod)** | `fan src/core/rustFolio/build.fan compile` |
| **Build (Rust binary)** | `cargo build --release` (in `rust/`) |
| **Gate** | `fant testFolio` |

> ⚠️ **The gate uses `target/release/rust-folio`** (not the dev build).
> After any Rust change, run `cargo build --release` before `fant testFolio`
> or failures will silently test stale code.

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
| S1 Socket authentication | 2026-02-23 | `372f141e` ✅ |
| Namespace reload hook | 2026-02-23 | `05e3b25d` ✅ |
| Code cleanup (deps + dead code + naming) | 2026-02-24 | `3c2d4c8f` + `7522c3a2`, 0 warnings ✅ |
| Performance benchmarks (Tier 0/1/2) | 2026-02-24 | `c37b4382`, full results in BENCHMARKS.md ✅ |

---

## Open Items

All production milestones, optimizations, security, and benchmarks are complete.
Remaining items are tracked in DESIGN.md §16 Future Work:

- **readAll projection** — reduce IPC transfer cost for high-cardinality queries
- **Scalability validation** — 50k–100k record benchmarks

---

## Notes

- Design rationale for all items above is in `DESIGN.md §16 Future Work`.
- Architectural decisions are in `DESIGN.md §15 Decision Log` (DEV-001 through DEV-019).
- `BENCHMARKS.md` contains methodology, all Tier 0/1/2 results, key findings, and the run log for all performance characterization work.
- Use `DESIGN.md` and `README.md` for permanent, upstream-mergeable content.
  PROGRESS.md is a working tracker and is intentionally not upstream-mergeable.
