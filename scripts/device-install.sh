#!/bin/sh
# Run by deploy.sh on the device. Never starts an application or pauses MainUI.
set -eu
task_deploy_dir=$1
task_sdcard=$2
task_expected_package=$3
trap 'task_status=$?; if [ "$task_status" -ne 0 ]; then printf "Deploy result: FAIL\n"; fi' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
cd "$task_deploy_dir"
task_actual=$(sha256sum package.tar.gz)
test "${task_actual%% *}" = "$task_expected_package"
mkdir unpacked
tar -xzf package.tar.gz -C unpacked

# Validate every destination and every staged binary before modifying any app.
while read -r task_port task_binary task_marker task_expected; do
    task_source="unpacked/Ports/$task_port"
    task_destination="$task_sdcard/Ports/$task_port"
    if [ -e "$task_destination" ] || [ -L "$task_destination" ]; then
        if [ ! -d "$task_destination" ] || [ -L "$task_destination" ] ||
            [ ! -f "$task_destination/$task_marker" ]; then
            printf 'Refusing to overwrite an unrecognized %s directory.\n' "$task_destination" >&2
            exit 1
        fi
    fi
    for task_file in "$task_binary" launch.sh test.sh config.json icon.png "$task_marker"; do
        test -f "$task_source/$task_file"
    done
    task_actual=$(sha256sum "$task_source/$task_binary")
    test "${task_actual%% *}" = "$task_expected"
done < manifest

while read -r task_port task_binary task_marker task_expected; do
    task_source="unpacked/Ports/$task_port"
    task_destination="$task_sdcard/Ports/$task_port"
    mkdir -p "$task_destination"
    # Mark a newly created app directory so an interrupted install can be retried.
    cp "$task_source/$task_marker" "$task_destination/$task_marker"
    for task_file in "$task_binary" launch.sh test.sh config.json icon.png; do
        cp "$task_source/$task_file" "$task_destination/$task_file"
    done
    chmod 755 "$task_destination/$task_binary" "$task_destination/launch.sh" "$task_destination/test.sh"
    task_actual=$(sha256sum "$task_destination/$task_binary")
    test "${task_actual%% *}" = "$task_expected"
    printf 'Installed: %s\n' "$task_destination"
done < manifest

if grep -q '^ChineseChess ' manifest && test -f "$task_sdcard/Apps/ChineseChess/.trimui-chinese-chess"; then
    rm -rf "$task_sdcard/Apps/ChineseChess"
    printf 'Removed old installation: %s\n' "$task_sdcard/Apps/ChineseChess"
fi
printf 'Deploy result: PASS\n'
