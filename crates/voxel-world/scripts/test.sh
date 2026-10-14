#!/bin/sh
# Reversible ADB test: pause MainUI, run under an independent watchdog,
# then restore CPU hotplug state and resume MainUI on every exit path.
cd "$(dirname "$0")" || exit 1
task_ui_pids="$(pidof MainUI)"
task_app_pid=
task_cpu_state=
for task_cpu in /sys/devices/system/cpu/cpu[1-9]*; do
    [ -f "$task_cpu/online" ] && task_cpu_state="$task_cpu_state ${task_cpu##*/cpu}:$(cat "$task_cpu/online")"
done
cleanup() {
    trap - EXIT HUP INT TERM
    if [ -n "$task_app_pid" ] && kill -0 "$task_app_pid" 2>/dev/null; then
        kill -TERM "$task_app_pid" 2>/dev/null
        wait "$task_app_pid" 2>/dev/null
    fi
    for task_entry in $task_cpu_state; do
        echo "${task_entry#*:}" > "/sys/devices/system/cpu/cpu${task_entry%%:*}/online" 2>/dev/null
    done
    for task_ui_pid in $task_ui_pids; do kill -CONT "$task_ui_pid" 2>/dev/null; done
    printf 'CPU online after exit: %s\n' "$(cat /sys/devices/system/cpu/online)"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
for task_ui_pid in $task_ui_pids; do kill -STOP "$task_ui_pid" || exit 1; done
if [ "$#" -eq 0 ]; then set -- --seconds 15 --save-dir /tmp/voxel-world-smoke --config-dir /tmp/voxel-world-smoke-config; fi
# The watchdog stays independent of the game's own --seconds limit.
task_limit="${VOXEL_TEST_LIMIT:-30}"
RUST_LOG="${RUST_LOG:-warn,trimui_voxel_world=info}" timeout -s TERM -k 5 "$task_limit" ./trimui-voxel-world "$@" &
task_app_pid=$!
wait "$task_app_pid"
task_result=$?
task_app_pid=
printf 'Voxel World exit status: %s\n' "$task_result"
exit "$task_result"
