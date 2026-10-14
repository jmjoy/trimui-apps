#!/bin/sh
# Writes known controller events directly to event3; do not use physical buttons during this test.
cd /mnt/SDCARD/Ports/VoxelWorld || exit 1
task_input_dir=/tmp/voxel-world-input
task_save=/tmp/voxel-world-input-save
task_config=/tmp/voxel-world-input-config
task_log=./voxel-world-input-run.log
task_helper_log=./voxel-world-input-helper.log
rm -rf "$task_save" "$task_config" /tmp/voxel-world-game-*.png
: > "$task_log"
: > "$task_helper_log"
task_cpu_before="$(cat /sys/devices/system/cpu/online)"
send() { cat "$task_input_dir/$1.bin" > /dev/input/event3; }
wait_for_count() {
    task_attempt=0
    while [ "$(grep -c "$1" "$task_log")" -lt "$2" ]; do
        task_attempt=$((task_attempt + 1))
        if [ "$task_attempt" -gt 100 ]; then printf 'Timed out waiting for: %s (%s)\n' "$1" "$2"; return 1; fi
        sleep 0.1
    done
}
wait_for() { wait_for_count "$1" 1; }
button() { send "$1-press"; sleep 0.12; send "$1-release"; sleep 0.2; }
# Both taps land inside the 300 ms double-tap window.
double() { send "$1-press"; sleep 0.05; send "$1-release"; sleep 0.07; send "$1-press"; sleep 0.05; send "$1-release"; sleep 0.25; }
# Firmware input daemons resample the sticks, so axis values are re-sent.
hold() {
    task_n=0
    while [ "$task_n" -lt "$2" ]; do send "$1"; sleep 0.03; task_n=$((task_n + 1)); done
    send neutral
    sleep 0.15
}
direction() { send "$1"; sleep 0.08; send neutral; sleep 0.2; }
repeat() { task_k=0; while [ "$task_k" -lt "$2" ]; do direction "$1"; task_k=$((task_k + 1)); done; }
shot() {
    kill -USR1 "$(pidof trimui-voxel-world)"
    wait_for "Screenshot: /tmp/voxel-world-game-$1.png"
}
(
    set -e
    trap 'send neutral; send exit-release' EXIT
    wait_for 'First frame: present completed'
    button a
    wait_for_count 'Game started' 1
    # Look at the ground in front of the player, not straight down.
    hold look-down 12
    hold dig 2
    wait_for 'Block broken'
    sleep 0.3
    hold place 2
    wait_for 'Block placed'
    button r
    wait_for 'Block selected: 2'
    button l
    wait_for 'Block selected: 1'
    button r
    wait_for_count 'Block selected: 2' 2
    button y
    wait_for 'Block picked: 1'
    button b
    wait_for 'Jump'
    double b
    wait_for 'Flight: true'
    hold move-forward 20
    hold look-right 15
    direction left
    wait_for 'Sneak toggle: true'
    direction right
    wait_for 'Sprint toggle: true'
    button x
    wait_for 'Palette opened'
    direction down
    direction right
    shot 1
    button a
    wait_for 'Palette pick:'
    wait_for_count 'Game started' 2
    # Place the picked block next to the player to show it in the world.
    hold place 2
    wait_for_count 'Block placed' 2
    # Slot 9 holds a glowstone lamp; light must spread from it.
    button l
    wait_for 'Block selected: 9 (glowstone)'
    # Aim a few blocks ahead so the lamp never overlaps the player.
    hold look-up 5
    hold place 2
    wait_for_count 'Block placed' 3
    button start
    wait_for 'Paused'
    repeat down 2
    button a
    direction right
    wait_for 'Settings: view=5'
    repeat down 8
    button a
    wait_for 'CPU cores: option=default'
    printf 'Device CPU online (default): %s\n' "$(cat /sys/devices/system/cpu/online)"
    button a
    wait_for 'CPU cores: option=all'
    printf 'Device CPU online (all): %s\n' "$(cat /sys/devices/system/cpu/online)"
    direction down
    button a
    wait_for 'debug_overlay=true'
    direction down
    button a
    button a
    wait_for 'Rebinding jump'
    shot 2
    button menu
    wait_for 'Binding: jump = menu'
    button b
    wait_for 'Config saved'
    button start
    wait_for_count 'Game started' 3
    double menu
    wait_for 'Flight: false'
    button start
    wait_for_count 'Paused' 2
    repeat down 2
    button a
    repeat down 10
    button a
    repeat down 10
    button a
    direction down
    button a
    wait_for 'Bindings reset to defaults'
    button start
    wait_for_count 'Game started' 4
    double b
    wait_for_count 'Flight: true' 2
    sleep 1.5
    shot 3
    send exit-press
    sleep 0.3
    send exit-release
) > "$task_helper_log" 2>&1 &
task_helper_pid=$!
trap 'kill "$task_helper_pid" 2>/dev/null; wait "$task_helper_pid" 2>/dev/null' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
VOXEL_TEST_LIMIT=60 sh ./test.sh --seconds 55 --save-dir "$task_save" --config-dir "$task_config" --seed 42 --capture /tmp/voxel-world-game.png --capture-after 999 > "$task_log" 2>&1
task_app_status=$?
wait "$task_helper_pid"
task_helper_status=$?
trap - EXIT
cat "$task_log"
cat "$task_helper_log"
printf 'Config file: %s\n' "$(tr -d ' \n' < "$task_config/settings.json")"
sleep 1
sh ./test.sh --seconds 4 --save-dir "$task_save" --config-dir "$task_config" > ./voxel-world-reload.log 2>&1
cat ./voxel-world-reload.log
task_saved="$(grep 'World saved:' "$task_log" | tail -n 1 | sed 's/.*digest=//')"
task_loaded="$(grep 'World loaded:' ./voxel-world-reload.log | sed 's/.*digest=//')"
printf 'Saved digest: %s; loaded digest: %s\n' "$task_saved" "$task_loaded"
if [ "$task_app_status" -eq 0 ] && [ "$task_helper_status" -eq 0 ] &&
    grep -q '^Voxel World exit status: 0' "$task_log" &&
    grep -q 'Exit requested by SELECT + START' "$task_log" &&
    grep -q 'Audio: 48000 Hz, 2 channels, F32' "$task_log" &&
    grep -q 'Audio bank ready' "$task_log" &&
    grep -q 'Audio callback:' "$task_log" &&
    grep -q 'Device CPU online (default): 0-1' "$task_helper_log" &&
    grep -q 'Device CPU online (all): 0-3' "$task_helper_log" &&
    grep -q "CPU online after exit: $task_cpu_before" "$task_log" &&
    tr -d ' \n' < "$task_config/settings.json" | grep -q '"jump":"b"' &&
    tr -d ' \n' < "$task_config/settings.json" | grep -q '"view_distance":5' &&
    tr -d ' \n' < "$task_config/settings.json" | grep -q '"debug_overlay":true' &&
    grep -q 'Config loaded' ./voxel-world-reload.log &&
    grep -q '^Voxel World exit status: 0' ./voxel-world-reload.log &&
    [ -n "$task_saved" ] && [ "$task_saved" = "$task_loaded" ] &&
    test -f /tmp/voxel-world-game-1.png && test -f /tmp/voxel-world-game-3.png; then
    printf '%s\n' 'Input test: PASS'
else
    printf '%s\n' 'Input test: FAIL'
    exit 1
fi
