#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
select_projects "${1:-all}"
entries=()
for project in "${projects[@]}"; do
    load_project "$project"
    # List installation files explicitly: never include logs, saves or stale apps.
    for file in "$binary_name" launch.sh test.sh config.json icon.png "$marker_name"; do
        entries+=("Ports/$port_name/$file")
    done
done
mkdir -p "$artifact_root"
temporary_package=$(mktemp "$artifact_root/.package.XXXXXX")
trap 'rm -f "$temporary_package"' EXIT
tar -czf "$temporary_package" -C "$artifact_root" -- "${entries[@]}"
chmod 644 "$temporary_package"
mv -f "$temporary_package" "$package_file"
printf 'Package: %s\n' "$package_file"
