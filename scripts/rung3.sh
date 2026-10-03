#!/usr/bin/env bash
# Rung 3: the replay manifests of the reference's MAME 0.285 lockstep runs
# (real game code with its interrupts), diffed in lockstep against the
# reference through a pipe. The manifests are made from ROMs and MAME traces,
# so they are never committed: build them in the reference with
#   python validation/lockstep.py export genesis ROM.zip   (and altbeast)
# Usage: scripts/rung3.sh MANIFEST [m68000-python checkout] [python]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MANIFEST=$1
REFERENCE=${2:-"$ROOT/external/m68000-python"}
PYTHON=${3:-"$REFERENCE/.venv-pypy/bin/pypy3"}
cargo build --release -q --manifest-path "$ROOT/Cargo.toml"
"$ROOT/target/release/m68k-trace" "$MANIFEST" \
    | PYTHONPATH="$REFERENCE/src" "$PYTHON" -m m68000_python.conformance diff "$MANIFEST" -
