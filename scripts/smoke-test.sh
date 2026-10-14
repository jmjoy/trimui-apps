#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
load_project "${1:?expected a project}"
mkdir -p "$artifact_dir"
args=(--seconds 15)
capture="/tmp/$slug-deploy.png"
if [ "$project" = voxel_world ]; then
    cpu_before=$(adb_shell cat /sys/devices/system/cpu/online | tr -d '\r')
    capture=/tmp/voxel-world-setup.png
    adb_shell rm -rf /tmp/voxel-world-smoke /tmp/voxel-world-smoke-config
    args+=(--save-dir /tmp/voxel-world-smoke --config-dir /tmp/voxel-world-smoke-config --seed 42)
fi
adb_shell rm -f "$capture"
adb_shell sh "$device_dir/test.sh" "${args[@]}" --capture "$capture" | tee "$artifact_dir/deploy-test.log"
grep -Fxq "$result_label exit status: 0" "$artifact_dir/deploy-test.log"
grep -q 'First frame: present completed' "$artifact_dir/deploy-test.log"
if [ "$project" = voxel_world ]; then
    grep -Fxq "CPU online after exit: $cpu_before" "$artifact_dir/deploy-test.log"
fi
adb pull "$capture" "$artifact_dir/setup.png" >/dev/null
printf 'Smoke test: PASS\n'
