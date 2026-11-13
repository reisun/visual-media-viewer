#!/bin/bash
set -euo pipefail
cd /app
test_dir="$PWD/target/thumbnail-tests"
mkdir -p "$test_dir/fixtures"
cp scripts/thumbnail-tests/Cargo.toml "$test_dir/Cargo.toml"
cp Cargo.lock "$test_dir/Cargo.lock"
cp scripts/thumbnail-tests/lib.rs "$test_dir/lib.rs"
export VMV_THUMBNAIL_FIXTURES="$test_dir/fixtures"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 1 -c:v libx264 -pix_fmt yuv420p -bf 2 "$VMV_THUMBNAIL_FIXTURES/bframes.mov"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 0.05 -c:v libx264 -pix_fmt yuv420p -bf 3 "$VMV_THUMBNAIL_FIXTURES/short.mp4"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i sine=frequency=440:sample_rate=48000 \
    -t 1 -c:a aac "$VMV_THUMBNAIL_FIXTURES/audio.m4a"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 1 -c:v mpeg2video -f mpegts "$VMV_THUMBNAIL_FIXTURES/fallback.ts"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=640x360:rate=24 \
    -f lavfi -i sine=frequency=440:sample_rate=48000 -t 8 -c:v libx264 \
    -preset ultrafast -crf 18 -c:a aac -movflags +faststart "$VMV_THUMBNAIL_FIXTURES/faststart.mp4"
export CARGO_TARGET_DIR="$test_dir/build"
export CARGO_HOME="$test_dir/cargo-home"
timeout 240s cargo test --manifest-path "$test_dir/Cargo.toml" \
    --target x86_64-unknown-linux-gnu -- --test-threads=1
