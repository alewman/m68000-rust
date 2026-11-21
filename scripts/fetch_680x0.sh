#!/usr/bin/env bash
# Fetch the 68000 part of SingleStepTests/680x0 (Tom Harte's corpus) at the
# revision m68000-python classifies. It has no license file: fetched for
# local use only, never committed.
# Usage: scripts/fetch_680x0.sh [destination]
set -euo pipefail
REVISION=e0d5ece9670205cc84a0101081837deb446f86a3
DEST=${1:-"$(dirname "$0")/../external/680x0"}
if [ -e "$DEST/.revision" ] && [ "$(cat "$DEST/.revision")" = "$REVISION" ]; then
    echo "SingleStepTests/680x0 already at $REVISION in $DEST"
    exit 0
fi
rm -rf "$DEST"
mkdir -p "$DEST"
git -C "$DEST" init -q
git -C "$DEST" sparse-checkout set --no-cone /68000/v1/
git -C "$DEST" fetch -q --depth 1 --filter=blob:none https://github.com/SingleStepTests/680x0.git "$REVISION"
git -C "$DEST" checkout -q --detach FETCH_HEAD
actual=$(git -C "$DEST" rev-parse HEAD)
[ "$actual" = "$REVISION" ] || { echo "revision mismatch: $actual" >&2; exit 1; }
echo "$REVISION" > "$DEST/.revision"
echo "Fetched SingleStepTests/680x0 (68000/v1) at $REVISION into $DEST"
