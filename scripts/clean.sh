#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
select_projects "${1:-all}"
cargo clean --manifest-path "$workspace_dir/Cargo.toml" "${cargo_scope[@]}"
for project in "${projects[@]}"; do
    load_project "$project"
    rm -rf "$stage_dir" "$artifact_root/$slug-trimui-smart-pro.tar.gz"
    if [ "$project" = chinese_chess ]; then
        for file in input input-events.cjs device-input-test.sh run-adb.log \
            deploy.log deploy-test.log input-test.log setup.png game.png; do
            rm -rf "$artifact_dir/$file"
        done
    else
        rm -rf "$artifact_dir"
    fi
done
# Any project cleanup invalidates the aggregate package.
rm -f "$artifact_root/trimui-apps-trimui-smart-pro.tar.gz"
