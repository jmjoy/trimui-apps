#!/bin/sh
# Reversible ADB test: pause MainUI, run for a bounded time, then resume it.
cd "$(dirname "$0")" || exit 1
task_ui_pids="$(pidof MainUI)"
task_demo_pid=
cleanup() {
    trap - EXIT HUP INT TERM
    if [ -n "$task_demo_pid" ] && kill -0 "$task_demo_pid" 2>/dev/null; then
        kill -TERM "$task_demo_pid" 2>/dev/null
        wait "$task_demo_pid" 2>/dev/null
    fi
    for task_ui_pid in $task_ui_pids; do kill -CONT "$task_ui_pid" 2>/dev/null; done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
for task_ui_pid in $task_ui_pids; do kill -STOP "$task_ui_pid" || exit 1; done
if [ "$#" -eq 0 ]; then set -- --seconds 15; fi
RUST_LOG="${RUST_LOG:-warn,trimui_gomoku=info}" timeout -s TERM -k 5 30 ./trimui-gomoku "$@" &
task_demo_pid=$!
wait "$task_demo_pid"
task_result=$?
task_demo_pid=
printf 'Gomoku exit status: %s\n' "$task_result"
exit "$task_result"
