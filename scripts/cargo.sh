#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
action=${1:?expected a Cargo task}
select_projects "${2:-all}"
cd "$workspace_dir"

case "$action" in
    fmt) cargo fmt --manifest-path "$workspace_dir/Cargo.toml" "${fmt_scope[@]}" -- --check ;;
    test) cargo test --manifest-path "$workspace_dir/Cargo.toml" --locked "${cargo_scope[@]}" ;;
    clippy) cargo clippy --manifest-path "$workspace_dir/Cargo.toml" --locked "${cargo_scope[@]}" --all-targets -- -D warnings ;;
    build)
        select_profile "${3:-release}"
        test -f "$toolchain_dir/pkgconfig/alsa.pc"
        test -f "$toolchain_dir/device-libs/libasound.so.2"
        PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR="$toolchain_dir/pkgconfig" \
            cargo zigbuild --locked --manifest-path "$workspace_dir/Cargo.toml" \
                --target-dir "$target_dir" --target "$target_triple" \
                "${cargo_scope[@]}" "${cargo_profile[@]}"
        for project in "${projects[@]}"; do
            load_project "$project"
            test -x "$target_dir/aarch64-unknown-linux-gnu/$output_profile/$binary_name"
        done
        ;;
    *) printf 'Unknown Cargo task: %s\n' "$action" >&2; exit 2 ;;
esac
