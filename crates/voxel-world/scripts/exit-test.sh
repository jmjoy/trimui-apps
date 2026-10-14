#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/common.sh"
load_project voxel_world
mkdir -p "$artifact_dir"
adb push "${project_dir}/scripts/device-exit-test.sh" /tmp/voxel-world-exit-test.sh >/dev/null
set +e
adb_shell sh /tmp/voxel-world-exit-test.sh | tee "${artifact_dir}/exit-test.log"
set -e
grep -q '^Exit tests: PASS' "${artifact_dir}/exit-test.log"
