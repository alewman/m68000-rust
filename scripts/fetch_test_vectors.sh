#!/usr/bin/env bash
# Fetch the exact SingleStepTests/m68000 revision m68000-python is certified against.
# Usage: scripts/fetch_test_vectors.sh [destination]
set -euo pipefail
REVISION=64b253116a3de04aaac4346c43680960dc9b67e5
DEST=${1:-"$(dirname "$0")/../external/m68000_test_vectors"}
if [ -e "$DEST/.revision" ] && [ "$(cat "$DEST/.revision")" = "$REVISION" ]; then
    echo "SingleStepTests/m68000 already at $REVISION in $DEST"
    exit 0
fi
rm -rf "$DEST"
mkdir -p "$DEST"
git -C "$DEST" init -q
git -C "$DEST" fetch -q --depth 1 https://github.com/SingleStepTests/m68000.git "$REVISION"
git -C "$DEST" checkout -q --detach FETCH_HEAD
actual=$(git -C "$DEST" rev-parse HEAD)
[ "$actual" = "$REVISION" ] || { echo "revision mismatch: $actual" >&2; exit 1; }
echo "$REVISION" > "$DEST/.revision"
echo "Fetched SingleStepTests/m68000 at $REVISION into $DEST"
