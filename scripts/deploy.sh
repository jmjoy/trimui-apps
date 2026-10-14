#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
select_projects "${1:-all}"
test -f "$package_file"
mkdir -p "$artifact_root"
manifest=$(mktemp "$artifact_root/.deploy-manifest.XXXXXX")
remote_dir=
cleanup() {
    rm -f "$manifest"
    if [ -n "$remote_dir" ]; then adb_shell rm -rf "$remote_dir" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
for project in "${projects[@]}"; do
    load_project "$project"
    digest=$(tar -xOzf "$package_file" "Ports/$port_name/$binary_name" | sha256sum)
    printf '%s %s %s %s\n' "$port_name" "$binary_name" "$marker_name" "${digest%% *}" >> "$manifest"
done
if [ "$selection" = all ]; then artifact_dir="$artifact_root"; fi
mkdir -p "$artifact_dir"
digest=$(sha256sum "$package_file")
adb wait-for-device
remote_candidate=$(adb_shell mktemp -d /mnt/SDCARD/.trimui-apps-deploy.XXXXXX | tr -d '\r')
if [[ ! "$remote_candidate" =~ ^/mnt/SDCARD/\.trimui-apps-deploy\.[[:alnum:]]{6}$ ]]; then
    printf 'Could not create deployment directory: %s\n' "$remote_candidate" >&2
    exit 1
fi
remote_dir=$remote_candidate
adb push "$package_file" "$remote_dir/package.tar.gz" >/dev/null
adb push "$manifest" "$remote_dir/manifest" >/dev/null
adb push "$workspace_dir/scripts/device-install.sh" "$remote_dir/install.sh" >/dev/null
adb_shell sh "$remote_dir/install.sh" "$remote_dir" /mnt/SDCARD "${digest%% *}" \
    | tee "$artifact_dir/deploy.log"
# The result line is required even if this adbd reports a zero exit status.
grep -Fxq 'Deploy result: PASS' "$artifact_dir/deploy.log"
