#!/bin/bash

set -xe

Xephyr -br -ac -noreset -glamor -screen 1920x1080 :1 &
XEPHYR_PID=$!

cargo build || exit 1

DISPLAY=:1 ./target/debug/toy-tiling-wm &
WM_PID=$!

trap "kill $XEPHYR_PID $WM_PID 2>/dev/null" EXIT
wait -n $XEPHYR_PID $WM_PID
