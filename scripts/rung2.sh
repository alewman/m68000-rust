#!/usr/bin/env bash
# Rung 2: all 127 SingleStepTests/m68000 files, 317,500 cases: registers, RAM,
# prefetch, bus transactions, clocks, and step_clocks at every access end.
# Usage: scripts/rung2.sh [corpus v1 directory]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CORPUS=${1:-"$ROOT/external/m68000_test_vectors/v1"}
cargo build --release -q --manifest-path "$ROOT/Cargo.toml"
exec "$ROOT/target/release/m68k-vectors" "$CORPUS"
