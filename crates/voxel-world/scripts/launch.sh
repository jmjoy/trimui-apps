#!/bin/sh
# MainUI yields its display before launching an app. The game brings CPU
# cores online itself; this wrapper restores the recorded hotplug state
# even after a crash or SIGKILL.
cd "$(dirname "$0")" || exit 1
export RUST_LOG="${RUST_LOG:-warn,trimui_voxel_world=info}"
if [ -f voxel-world.log ] && [ "$(wc -c < voxel-world.log)" -gt 2000000 ]; then
    mv -f voxel-world.log voxel-world.log.old
fi
task_cpu_state=
for task_cpu in /sys/devices/system/cpu/cpu[1-9]*; do
    [ -f "$task_cpu/online" ] && task_cpu_state="$task_cpu_state ${task_cpu##*/cpu}:$(cat "$task_cpu/online")"
done
restore_cpus() {
    for task_entry in $task_cpu_state; do
        echo "${task_entry#*:}" > "/sys/devices/system/cpu/cpu${task_entry%%:*}/online" 2>/dev/null
    done
}
task_app_pid=
forward() { [ -n "$task_app_pid" ] && kill -TERM "$task_app_pid" 2>/dev/null; }
trap forward HUP INT TERM
./trimui-voxel-world "$@" >> voxel-world.log 2>&1 &
task_app_pid=$!
while :; do
    wait "$task_app_pid"
    task_result=$?
    kill -0 "$task_app_pid" 2>/dev/null || break
done
restore_cpus
exit "$task_result"
