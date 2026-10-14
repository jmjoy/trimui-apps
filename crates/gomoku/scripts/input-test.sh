#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/common.sh"
load_project gomoku
mkdir -p "$artifact_dir"
printf '%s\n' 'Input test writes event3: do not operate the physical controller during the test.'
input_dir="${artifact_dir}/input"
rm -rf "$input_dir"
node "${project_dir}/scripts/input-events.cjs" "$input_dir"
adb wait-for-device
adb push "$input_dir" /tmp/gomoku-input >/dev/null
adb push "${project_dir}/scripts/device-input-test.sh" /tmp/gomoku-input-test.sh >/dev/null
# Let MainUI finish reopening its audio route before pausing it again.
sleep 2
set +e
adb_shell sh /tmp/gomoku-input-test.sh | tee "${artifact_dir}/input-test.log"
set -e
grep -q '^Input test: PASS' "${artifact_dir}/input-test.log"
adb pull /tmp/gomoku-game.png "${artifact_dir}/game.png" >/dev/null
GOMOKU_DEVICE_SMOKE_PNG="${artifact_dir}/game.png" \
    cargo test --manifest-path "${workspace_dir}/Cargo.toml" --locked \
        -p "${binary_name}" device_capture_contains_board_and_both_stones -- --ignored
