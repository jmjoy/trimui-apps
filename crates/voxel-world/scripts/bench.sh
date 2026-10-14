#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/common.sh"
load_project voxel_world
seconds=${1:-40}
if [ "$#" -gt 0 ]; then shift; fi
if [ "${1:-}" = -- ]; then shift; fi
if [[ ! "$seconds" =~ ^[1-9][0-9]{0,8}$ ]]; then
    printf 'bench seconds must be a positive integer (at most 9 digits)\n' >&2
    exit 2
fi
limit=$((10#$seconds + 30))
mkdir -p "$artifact_dir"
adb_shell rm -f /tmp/voxel-world-bench.png
adb_shell env "VOXEL_TEST_LIMIT=$limit" sh "$device_dir/test.sh" --bench "$seconds" \
    --save-dir /tmp/voxel-world-bench --capture /tmp/voxel-world-bench.png --capture-after 15 "$@" \
    | tee "$artifact_dir/bench.log"
grep -Fxq 'Voxel World exit status: 0' "$artifact_dir/bench.log"
result=$(grep -o 'Bench result:.*' "$artifact_dir/bench.log" | tail -n 1)
test -n "$result"
adb pull /tmp/voxel-world-bench.png "$artifact_dir/bench.png" >/dev/null
revision=$(git -C "$workspace_dir" rev-parse --short HEAD 2>/dev/null || echo unknown)
dirty=$(git -C "$workspace_dir" diff --quiet 2>/dev/null || echo +dirty)
printf '%s %s%s %s\n' "$(date -Iseconds)" "$revision" "$dirty" "$result" >> "$artifact_dir/bench-history.log"
printf '%s\n' "$result"
