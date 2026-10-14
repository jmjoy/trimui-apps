#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
select_projects "${1:-all}"
select_profile "${2:-release}"

for project in "${projects[@]}"; do
    load_project "$project"
    test -x "$target_dir/aarch64-unknown-linux-gnu/$output_profile/$binary_name"
    mkdir -p "$artifact_dir"
    rm -rf "$stage_dir"
    mkdir -p "$stage_dir"
    cp "$target_dir/aarch64-unknown-linux-gnu/$output_profile/$binary_name" "$stage_dir/"
    cp "$project_dir/config.json" "$project_dir/icon.png" "$stage_dir/"
    cp "$project_dir/scripts/launch.sh" "$project_dir/scripts/test.sh" "$stage_dir/"
    : > "$stage_dir/$marker_name"
    chmod 755 "$stage_dir/$binary_name" "$stage_dir/launch.sh" "$stage_dir/test.sh"
done
