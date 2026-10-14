#!/bin/sh
# MainUI yields its display before launching an app.
cd "$(dirname "$0")" || exit 1
export RUST_LOG="${RUST_LOG:-warn,trimui_gomoku=info}"
exec ./trimui-gomoku "$@" >> gomoku.log 2>&1
