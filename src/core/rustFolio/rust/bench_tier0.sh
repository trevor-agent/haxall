#!/usr/bin/env bash
# bench_tier0.sh — capture rustFolio build metrics (Tier 0)
# Run from rust/ directory: ./bench_tier0.sh
set -e

echo "## Tier 0 — Build Metrics"
echo ""
echo "Platform: $(uname -ms)"
echo "Date:     $(date)"
echo ""

echo "### Binary size"
if [ -f target/release/rust-folio ]; then
  ls -lh target/release/rust-folio | awk '{print "  Unstripped: " $5}'
  if command -v strip &>/dev/null; then
    cp target/release/rust-folio /tmp/rust-folio-stripped
    strip /tmp/rust-folio-stripped
    ls -lh /tmp/rust-folio-stripped | awk '{print "  Stripped:   " $5}'
    rm /tmp/rust-folio-stripped
  fi
else
  echo "  (binary not built — run cargo build --release first)"
fi
echo ""

echo "### Dependencies"
echo -n "  Direct:     "
cargo tree --depth 1 2>/dev/null | tail -n +2 | grep -c "├\|└" || echo "unknown"
echo -n "  Transitive: "
cargo tree 2>/dev/null | grep -c "^[a-z]" || echo "unknown"
echo ""

echo "### Compile time (clean release build)"
cargo clean -q
time cargo build --release -q
echo ""
