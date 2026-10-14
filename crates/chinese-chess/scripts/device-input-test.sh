#!/bin/sh
# Development smoke test; events come from input-events.cjs, not a new uinput device.
cd /mnt/SDCARD/Ports/ChineseChess || exit 1
task_input_dir=/tmp/chinese-chess-input
task_log=./chinese-chess-input-test.log
: > "$task_log"
rm -f /tmp/chinese-chess-game.png
wait_for_text() {
    task_attempt=0
    while ! grep -q "$1" "$task_log"; do
        task_attempt=$((task_attempt + 1))
        [ "$task_attempt" -le 100 ] || return 1
        sleep 0.1
    done
}
button() { cat "$task_input_dir/$1.bin" > /dev/input/event3; sleep 0.15; }
direction() {
    cat "$task_input_dir/$1.bin" > /dev/input/event3
    sleep 0.08
    cat "$task_input_dir/neutral.bin" > /dev/input/event3
    sleep 0.12
}
(
    trap 'cat "$task_input_dir/neutral.bin" "$task_input_dir/exit-release.bin" > /dev/input/event3' EXIT
    wait_for_text 'First frame: present completed' || exit 1
    button start
    wait_for_text 'Game started: Red, Normal' || exit 1
    button a
    direction up
    button a
    wait_for_text 'Move 2: Black' || exit 1
    sleep 4
    button x
    wait_for_text 'Undo: 0 moves remain' || exit 1
    button start
    direction down
    direction down
    direction down
    direction down
    button a
    wait_for_text 'Returned to setup' || exit 1
    direction right
    button start
    wait_for_text 'Move 1: Red' || exit 1
    button a
    direction down
    button a
    wait_for_text 'Move 3: Red' || exit 1
    button x
    wait_for_text 'Undo: 1 moves remain' || exit 1
    cat "$task_input_dir/exit-press.bin" > /dev/input/event3
    sleep 0.2
    cat "$task_input_dir/exit-release.bin" > /dev/input/event3
) &
task_helper_pid=$!
sh ./test.sh --seconds 25 --capture /tmp/chinese-chess-game.png --capture-after 4 > "$task_log" 2>&1
task_app_status=$?
wait "$task_helper_pid"
task_helper_status=$?
cat "$task_log"
if [ "$task_app_status" -eq 0 ] && [ "$task_helper_status" -eq 0 ] &&
    grep -q 'Exit requested by SELECT + START' "$task_log" &&
    grep -q 'Audio: 48000 Hz' "$task_log"; then
    printf '%s\n' 'Input test: PASS'
else
    printf '%s\n' 'Input test: FAIL'
    exit 1
fi
