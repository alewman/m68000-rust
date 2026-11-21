#!/usr/bin/env bash
# Check out the m68000-python reference at the pinned commit and install it
# into a virtual environment, so the rung 1/3/5 diffs run against exactly
# that core and conformance kit.
# Usage: scripts/fetch_m68000python.sh [destination]
set -euo pipefail
# 4454d54: m68000-python's history reset (2026-11-20), whose tree is the removed
# dd93178's: 27c3792 plus one docs/provenance.md line, so the gates are unchanged.
COMMIT=${M68000_PYTHON_COMMIT:-4454d5457842fabe0fac92272589945c1eed7234}
DEST=${1:-"$(dirname "$0")/../external/m68000-python"}
PYTHON=${PYTHON:-python3}
if [ ! -d "$DEST/.git" ]; then
    git clone -q https://github.com/alewman/m68000-python.git "$DEST"
fi
git -C "$DEST" fetch -q origin
git -C "$DEST" checkout -q --detach "$COMMIT"
if [ ! -x "$DEST/.venv/bin/python" ]; then
    "$PYTHON" -m venv "$DEST/.venv"
fi
"$DEST/.venv/bin/python" -m pip install -q -e "$DEST"
echo "m68000-python at $(git -C "$DEST" rev-parse --short HEAD) installed in $DEST/.venv"
