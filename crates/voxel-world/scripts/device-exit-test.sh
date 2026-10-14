#!/bin/sh
# Exit paths: error, TERM, KILL (test.sh and launch.sh), watchdog. Each must
# restore the CPU hotplug state and leave MainUI running.
cd /mnt/SDCARD/Ports/VoxelWorld || exit 1
task_ui_pids="$(pidof MainUI)"
test -n "$task_ui_pids" || { printf 'MainUI is not running\n'; exit 1; }
task_cpu_before="$(cat /sys/devices/system/cpu/online)"
task_save=/tmp/voxel-world-exit-save
task_config=/tmp/voxel-world-exit-config
rm -rf "$task_save" "$task_config"
check() {
    task_now="$(cat /sys/devices/system/cpu/online)"
    if [ "$task_now" != "$task_cpu_before" ]; then printf '%s: CPU online %s, expected %s\n' "$1" "$task_now" "$task_cpu_before"; exit 1; fi
    for task_ui_pid in $task_ui_pids; do
        if grep -q '^State:.*T' "/proc/$task_ui_pid/status"; then printf '%s: MainUI still stopped\n' "$1"; exit 1; fi
    done
    printf '%s recovery: PASS\n' "$1"
}
wait_frame() {
    task_attempt=0
    until grep -q 'First frame: present completed' "$1"; do
        task_attempt=$((task_attempt + 1))
        [ "$task_attempt" -lt 100 ] || exit 1
        sleep 0.1
    done
    sleep 1
}
sh ./test.sh --seconds 1 --controller /dev/input/missing-voxel-test --save-dir "$task_save" --config-dir "$task_config" > ./voxel-world-error.log 2>&1
cat ./voxel-world-error.log
grep -q '^Voxel World exit status: 1' ./voxel-world-error.log || exit 1
check Error
sleep 2
for task_signal in TERM KILL; do
    task_log="./voxel-world-$task_signal.log"
    : > "$task_log"
    (
        wait_frame "$task_log"
        printf 'CPU online while running: %s\n' "$(cat /sys/devices/system/cpu/online)"
        kill "-$task_signal" "$(pidof trimui-voxel-world)"
    ) > "./voxel-world-$task_signal-helper.log" 2>&1 &
    task_helper_pid=$!
    sh ./test.sh --seconds 0 --save-dir "$task_save" --config-dir "$task_config" --seed 52 > "$task_log" 2>&1
    wait "$task_helper_pid" || exit 1
    cat "$task_log" "./voxel-world-$task_signal-helper.log"
    grep -q 'CPU online while running: 0-3' "./voxel-world-$task_signal-helper.log" || exit 1
    if [ "$task_signal" = TERM ]; then
        grep -q '^Voxel World exit status: 0' "$task_log" || exit 1
        grep -q 'World saved:' "$task_log" || exit 1
    else
        grep -q '^Voxel World exit status: 137' "$task_log" || exit 1
    fi
    check "$task_signal"
    sleep 2
done
# launch.sh is what MainUI runs; it must restore cores after a SIGKILL too.
for task_ui_pid in $task_ui_pids; do kill -STOP "$task_ui_pid"; done
trap 'for task_ui_pid in $task_ui_pids; do kill -CONT "$task_ui_pid"; done' EXIT
task_log_size="$(wc -c < voxel-world.log 2>/dev/null || echo 0)"
sh ./launch.sh --seconds 0 --save-dir "$task_save" --config-dir "$task_config" &
task_launch_pid=$!
task_attempt=0
until tail -c "+$((task_log_size + 1))" voxel-world.log 2>/dev/null | grep -q 'First frame: present completed'; do
    task_attempt=$((task_attempt + 1))
    [ "$task_attempt" -lt 100 ] || exit 1
    sleep 0.1
done
sleep 1
printf 'launch.sh CPU online while running: %s\n' "$(cat /sys/devices/system/cpu/online)"
kill -KILL "$(pidof trimui-voxel-world)"
wait "$task_launch_pid"
for task_ui_pid in $task_ui_pids; do kill -CONT "$task_ui_pid"; done
trap - EXIT
check launch.sh-KILL
sleep 2
task_started="$(date +%s)"
sh ./test.sh --seconds 0 --save-dir "$task_save" --config-dir "$task_config" > ./voxel-world-timeout.log 2>&1
task_elapsed=$(($(date +%s) - task_started))
cat ./voxel-world-timeout.log
printf 'Watchdog elapsed: %s seconds\n' "$task_elapsed"
[ "$task_elapsed" -ge 29 ] && [ "$task_elapsed" -le 37 ] || exit 1
# This firmware's BusyBox may return the child's own status after TERM.
grep -Eq '^Voxel World exit status: (0|124)' ./voxel-world-timeout.log || exit 1
grep -q 'World saved:' ./voxel-world-timeout.log || exit 1
check Watchdog
printf '%s\n' 'Exit tests: PASS'
