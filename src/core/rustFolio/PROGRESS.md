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
| M1 - Basic CRUD | 🔲 pending | | BasicTest partial (CRUD, reopen, curVer) |
| M2 - Filters | 🔲 pending | | BasicTest full (filters, readOpts, kinds), PrefixTest |
| M3 - Transient | 🔲 pending | | BasicTest (transient), trash |
| M4 - Hooks | 🔲 pending | | BasicTest (hooks) |
| M5 - History | 🔲 pending | | HisTest |
| M6 - Display | 🔲 pending | | DisTest |
| M7 - Full Green | 🔲 pending | | All testFolio green for rustfolio impl |

## Deviations from Reference Design

### DEV-001 — Index registration approach
**Reference:** The reference project notes that the `testFolio.impl` index entry in `build.fan`
should be deferred to M2 (commented out), and `testFolio/build.fan` should not add `rustFolio`
to its depends until M2. The reasoning: `AbstractFolioTest.runImpls` iterates all impls via
`each{}` without per-impl exception isolation, so stub UnsupportedErrs propagate and prevent
`flatfile`/`hx` from running in the same test method.

**This implementation:** Follows the reference exactly. Index registration is commented out in
`build.fan`. Will be enabled at M2 once commits+reads are functional.

### DEV-002 — Added to haxall/src/core/build.fan
**Reference:** The reference project lives as a separate parent workspace (not integrated into
the main haxall src/core/build.fan). This implementation adds `rustFolio/build.fan` to
`src/core/build.fan` so it's included in standard `./build.sh haxall` builds.

**Impact:** None on test results. Enables `rustFolio.pod` to be rebuilt automatically with
the rest of haxall.

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
