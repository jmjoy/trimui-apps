#!/bin/sh
# Writes known controller events directly to event3; do not use physical buttons during this test.
cd /mnt/SDCARD/Ports/Gomoku || exit 1
task_input_dir=/tmp/gomoku-input
task_log=/mnt/SDCARD/Ports/Gomoku/input-test.log
: > "$task_log"
rm -f /tmp/gomoku-game.png
wait_for_count() {
    task_attempt=0
    while [ "$(grep -c "$1" "$task_log")" -lt "$2" ]; do
        task_attempt=$((task_attempt + 1))
        [ "$task_attempt" -le 100 ] || return 1
        sleep 0.1
    done
}
wait_for_text() { wait_for_count "$1" 1; }
button() { cat "$task_input_dir/$1.bin" > /dev/input/event3; sleep 0.15; }
direction() {
    cat "$task_input_dir/$1.bin" > /dev/input/event3
    sleep 0.08
    cat "$task_input_dir/neutral.bin" > /dev/input/event3
    sleep 0.12
}
(
    set -e
    trap 'cat "$task_input_dir/neutral.bin" "$task_input_dir/exit-release.bin" > /dev/input/event3' EXIT
    wait_for_text 'First frame: present completed'
    button start
    wait_for_text 'Game started: Black, Normal'
    button y
    wait_for_count 'Hint ready:' 1
    button a
    wait_for_text 'Move 2: White'
    sleep 4
    button x
    wait_for_text 'Undo: 0 moves remain'
    button start
    direction down
    direction down
    direction down
    direction down
    direction down
    direction right
    wait_for_text 'Audio settings: muted=true'
    direction down
    direction right
    direction down
    direction right
    direction down
    direction right
    direction down
    direction right
    wait_for_text 'effects_volume=70%, music_volume=30%'
    direction up
    direction up
    direction up
    direction up
    direction right
    direction down
    direction right
    direction down
    direction down
    direction right
    wait_for_text 'muted=false, effects=true, music=true, effects_volume=70%, music_volume=30%'
    direction down
    direction down
    button a
    wait_for_text 'Returned to setup'
    direction right
    button start
    wait_for_text 'Game started: White, Normal'
    wait_for_count 'Move 1: Black' 2
    button y
    wait_for_count 'Hint ready:' 2
    button a
    wait_for_text 'Move 3: Black'
    button x
    wait_for_text 'Undo: 1 moves remain'
    cat "$task_input_dir/exit-press.bin" > /dev/input/event3
    sleep 0.2
    cat "$task_input_dir/exit-release.bin" > /dev/input/event3
) &
task_helper_pid=$!
trap 'kill "$task_helper_pid" 2>/dev/null; wait "$task_helper_pid" 2>/dev/null' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
sh ./test.sh --seconds 25 --capture /tmp/gomoku-game.png --capture-after 4 > "$task_log" 2>&1
task_app_status=$?
wait "$task_helper_pid"
task_helper_status=$?
trap - EXIT
cat "$task_log"
if [ "$task_app_status" -eq 0 ] && [ "$task_helper_status" -eq 0 ] &&
    grep -q '^Gomoku exit status: 0' "$task_log" &&
    grep -q 'Exit requested by SELECT + START' "$task_log" &&
    grep -q 'Audio: 48000 Hz, 2 channels, F32' "$task_log" &&
    test -f /tmp/gomoku-game.png; then
    printf '%s\n' 'Input test: PASS'
else
    printf '%s\n' 'Input test: FAIL'
    exit 1
fi
