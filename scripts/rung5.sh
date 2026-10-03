#!/usr/bin/env bash
# Rung 5: the interrupt and STOP scenarios as manifests with events, diffed
# against the reference.
# Usage: scripts/rung5.sh [m68000-python checkout] [python]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
REFERENCE=${1:-"$ROOT/external/m68000-python"}
PYTHON=${2:-"$REFERENCE/.venv/bin/python"}
cargo build --release -q --manifest-path "$ROOT/Cargo.toml"
status=0
count=0
for manifest in "$REFERENCE"/examples/conformance/interrupts/*.json; do
    "$ROOT/target/release/m68k-trace" "$manifest" 2>/dev/null \
        | PYTHONPATH="$REFERENCE/src" "$PYTHON" -m m68000_python.conformance diff "$manifest" - \
        || status=1
    count=$((count + 1))
done
echo "$count interrupt manifests diffed"
exit $status
