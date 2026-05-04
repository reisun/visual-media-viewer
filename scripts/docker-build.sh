#!/bin/bash
set -e

PROFILE="${1:-debug}"

if [ "$PROFILE" = "check" ]; then
    cargo check --target x86_64-pc-windows-gnu
    exit 0
elif [ "$PROFILE" = "release" ]; then
    cargo build --release --target x86_64-pc-windows-gnu
    OUT_DIR="/app/target/x86_64-pc-windows-gnu/release"
else
    cargo build --target x86_64-pc-windows-gnu
    OUT_DIR="/app/target/x86_64-pc-windows-gnu/debug"
fi

cp /opt/ffmpeg-win64/bin/*.dll "$OUT_DIR/"
