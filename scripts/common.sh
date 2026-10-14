#!/usr/bin/env bash
# Shared host-side paths and project metadata. Source from Bash scripts.
workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_dir="$workspace_dir/target"
artifact_root="$workspace_dir/artifacts"
toolchain_dir="$workspace_dir/toolchain"
target_triple=aarch64-unknown-linux-gnu.2.33

load_project() {
    project=$1
    case "$project" in
        chinese_chess) slug=chinese-chess; port_name=ChineseChess; result_label='Chinese Chess' ;;
        gomoku) slug=gomoku; port_name=Gomoku; result_label=Gomoku ;;
        voxel_world) slug=voxel-world; port_name=VoxelWorld; result_label='Voxel World' ;;
        *) printf 'Unknown project: %s (expected all, chinese_chess, gomoku or voxel_world)\n' "$project" >&2; return 2 ;;
    esac
    project_dir="$workspace_dir/crates/$slug"
    binary_name="trimui-$slug"
    marker_name=".$binary_name"
    stage_dir="$artifact_root/Ports/$port_name"
    device_dir="/mnt/SDCARD/Ports/$port_name"
    artifact_dir="$artifact_root/$slug"
    # Preserve the original Chinese Chess log and screenshot paths.
    if [ "$project" = chinese_chess ]; then artifact_dir="$artifact_root"; fi
}

select_projects() {
    selection=$1
    if [ "$selection" = all ]; then
        projects=(chinese_chess gomoku voxel_world)
        cargo_scope=(--workspace)
        fmt_scope=(--all)
        package_file="$artifact_root/trimui-apps-trimui-smart-pro.tar.gz"
    else
        load_project "$selection"
        projects=("$selection")
        cargo_scope=(-p "$binary_name")
        fmt_scope=(-p "$binary_name")
        package_file="$artifact_root/$slug-trimui-smart-pro.tar.gz"
    fi
}

select_profile() {
    output_profile=$1
    case "$output_profile" in
        release) cargo_profile=(--release) ;;
        debug) cargo_profile=() ;;
        *) printf 'profile must be release or debug\n' >&2; return 2 ;;
    esac
}

# Old adbd joins shell arguments without quoting. Quote each argument ourselves.
adb_shell() {
    local command= arg
    for arg in "$@"; do
        command+=" '${arg//\'/\'\\\'\'}'"
    done
    adb shell "$command" | tr -d '\r'
}
