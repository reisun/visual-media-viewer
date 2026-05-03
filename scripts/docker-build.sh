#!/bin/bash
set -e
cargo build --release --target x86_64-pc-windows-gnu
cp /opt/ffmpeg-win64/bin/*.dll /app/target/x86_64-pc-windows-gnu/release/
