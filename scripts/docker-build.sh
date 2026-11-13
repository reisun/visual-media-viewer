#!/bin/bash
set -e

PROFILE="${1:-debug}"

if [ "$PROFILE" = "check" ]; then
    cargo check --target x86_64-pc-windows-gnu
    cargo check --manifest-path shell-thumbnail/Cargo.toml --target-dir /app/target --target x86_64-pc-windows-gnu
    exit 0
elif [ "$PROFILE" = "release" ]; then
    cargo build --release --target x86_64-pc-windows-gnu
    OUT_DIR="/app/target/x86_64-pc-windows-gnu/release"
    cargo build --manifest-path shell-thumbnail/Cargo.toml --target-dir /app/target --release --target x86_64-pc-windows-gnu
else
    cargo build --target x86_64-pc-windows-gnu
    OUT_DIR="/app/target/x86_64-pc-windows-gnu/debug"
    cargo build --manifest-path shell-thumbnail/Cargo.toml --target-dir /app/target --target x86_64-pc-windows-gnu
fi

cp /opt/ffmpeg-win64/bin/*.dll "$OUT_DIR/"
