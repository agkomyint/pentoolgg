#!/usr/bin/env sh
# Usage: check-binary-size.sh <binary> <budget-bytes>
# Prints the size and fails when the release binary exceeds the budget in docs/performance.md.
set -eu
size=$(wc -c < "$1" | tr -d ' ')
echo "binary-size bytes=$size budget=$2 file=$1"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "| $1 | $size | $2 |" >> "$GITHUB_STEP_SUMMARY"
[ "$size" -le "$2" ] || { echo "binary exceeds size budget" >&2; exit 1; }
