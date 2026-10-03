#!/usr/bin/env bash
# Rung 4: ABCD, SBCD and NBCD over every input against flamewing's
# hardware-verified table (T1), by SHA-256.
# Usage: scripts/rung4.sh [bcd-table.bin, for a case-by-case report]
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cargo build --release -q --manifest-path "$ROOT/Cargo.toml"
exec "$ROOT/target/release/m68k-bcd" "$@"
