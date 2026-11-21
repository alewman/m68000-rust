#!/usr/bin/env bash
# Rung 6: SingleStepTests/680x0 as a detector. This core's classification of
# every disagreement must be the reference's, line for line: the reference's
# scripts/classify_680x0.py and m68k-680x0 both print the table, and the two
# are diffed.
# Usage: scripts/rung6.sh [corpus 68000/v1 directory] [m68000-python checkout] [python]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CORPUS=${1:-"$ROOT/external/680x0/68000/v1"}
REFERENCE=${2:-"$ROOT/external/m68000-python"}
PYTHON=${3:-"$REFERENCE/.venv/bin/python"}
cargo build --release -q --manifest-path "$ROOT/Cargo.toml"
ours=$(mktemp)
theirs=$(mktemp)
trap 'rm -f "$ours" "$theirs"' EXIT
"$ROOT/target/release/m68k-680x0" "$CORPUS" 2>/dev/null > "$ours"
# The reference reads the corpus from its own tests/ directory.
mkdir -p "$REFERENCE/tests/68000_test_vectors/680x0/68000"
[ -e "$REFERENCE/tests/68000_test_vectors/680x0/68000/v1" ] \
    || ln -s "$(cd "$CORPUS" && pwd)" "$REFERENCE/tests/68000_test_vectors/680x0/68000/v1"
"$PYTHON" "$REFERENCE/scripts/classify_680x0.py" 2>/dev/null > "$theirs"
cat "$ours"
if diff "$theirs" "$ours"; then
    echo "rung 6: the classification is the reference's, line for line"
else
    echo "rung 6: the classification differs from the reference's" >&2
    exit 1
fi
