#!/bin/sh
# MainUI yields its display before launching an app.
cd "$(dirname "$0")" || exit 1
export RUST_LOG="${RUST_LOG:-warn,trimui_chinese_chess=info}"
exec ./trimui-chinese-chess "$@" >> chess.log 2>&1
