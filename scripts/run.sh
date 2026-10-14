#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
load_project "${1:?expected a project}"
shift
if [ "${1:-}" = -- ]; then shift; fi
mkdir -p "$artifact_dir"
args=("$@")
save_args=()
capture=
capture_next=false
if [ "$project" = voxel_world ]; then
    save_args=(--save-dir /tmp/voxel-world-smoke --config-dir /tmp/voxel-world-smoke-config)
fi
for arg in "${args[@]}"; do
    if "$capture_next"; then capture=$arg; capture_next=false; fi
    case "$arg" in
        --save-dir|--save-dir=*|--config-dir|--config-dir=*) save_args=() ;;
        --capture) capture_next=true ;;
        --capture=*) capture=${arg#--capture=} ;;
    esac
done
adb wait-for-device
if [ -n "$capture" ]; then adb_shell rm -f "$capture"; fi
adb_shell sh "$device_dir/test.sh" "${save_args[@]}" "${args[@]}" | tee "$artifact_dir/run-adb.log"
grep -Fxq "$result_label exit status: 0" "$artifact_dir/run-adb.log"
probe=false
for arg in "${args[@]}"; do if [ "$arg" = --probe ]; then probe=true; fi; done
if ! "$probe"; then grep -q 'First frame: present completed' "$artifact_dir/run-adb.log"; fi
if [ -n "$capture" ]; then
    capture_file=setup.png
    if [ "$project" = voxel_world ]; then capture_file=capture.png; fi
    adb pull "$capture" "$artifact_dir/$capture_file" >/dev/null
fi
