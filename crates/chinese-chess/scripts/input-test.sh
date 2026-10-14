#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/common.sh"
load_project chinese_chess
mkdir -p "$artifact_dir"
printf '%s\n' 'Input test writes event3: do not operate the physical controller during this test.'
input_dir="${artifact_dir}/input"
rm -rf "$input_dir"
node "${project_dir}/scripts/input-events.cjs" "$input_dir"
adb wait-for-device
adb push "$input_dir" /tmp/chinese-chess-input >/dev/null
adb push "${project_dir}/scripts/device-input-test.sh" /tmp/chinese-chess-input-test.sh >/dev/null
# Let MainUI finish reopening its audio route after the smoke test.
sleep 2
set +e
adb_shell sh /tmp/chinese-chess-input-test.sh | tee "${artifact_dir}/input-test.log"
set -e
grep -q '^Input test: PASS' "${artifact_dir}/input-test.log"
adb pull /tmp/chinese-chess-game.png "${artifact_dir}/game.png" >/dev/null
