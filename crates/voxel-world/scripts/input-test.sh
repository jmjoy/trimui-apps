#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/common.sh"
load_project voxel_world
mkdir -p "$artifact_dir"
printf '%s\n' 'Input test writes event3: do not operate the physical controller during this test.'
node "${project_dir}/scripts/input-events.cjs" "${artifact_dir}/input"
adb_shell rm -rf /tmp/voxel-world-input
adb push "${artifact_dir}/input" /tmp/voxel-world-input >/dev/null
adb push "${project_dir}/scripts/device-input-test.sh" /tmp/voxel-world-input-test.sh >/dev/null
# Let MainUI finish reopening its audio route before pausing it again.
sleep 2
set +e
adb_shell sh /tmp/voxel-world-input-test.sh | tee "${artifact_dir}/input-test.log"
set -e
grep -q '^Input test: PASS' "${artifact_dir}/input-test.log"
adb pull /tmp/voxel-world-game-1.png "${artifact_dir}/palette.png" >/dev/null
adb pull /tmp/voxel-world-game-2.png "${artifact_dir}/controls.png" >/dev/null
adb pull /tmp/voxel-world-game-3.png "${artifact_dir}/game.png" >/dev/null
