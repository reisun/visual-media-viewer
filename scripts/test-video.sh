#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
test_dir="$PWD/target/video-tests"
mkdir -p "$test_dir/fixtures"
cp scripts/video-tests/Cargo.toml "$test_dir/Cargo.toml"
cp Cargo.lock "$test_dir/Cargo.lock"
cp scripts/video-tests/lib.rs "$test_dir/lib.rs"

export VMV_VIDEO_FIXTURES="$test_dir/fixtures"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 4 -c:v libx264 -pix_fmt yuv420p -g 24 -bf 0 "$VMV_VIDEO_FIXTURES/silent.mp4"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -f lavfi -i sine=frequency=440:sample_rate=48000 -t 4 \
    -c:v libx264 -pix_fmt yuv420p -g 24 -bf 2 -c:a aac "$VMV_VIDEO_FIXTURES/audio.mp4"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 4 -c:v libx264 -pix_fmt yuv420p -g 48 -bf 3 "$VMV_VIDEO_FIXTURES/bframes.mov"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=160x96:rate=24 \
    -t 0.2 -c:v libx264 -pix_fmt yuv420p -bf 0 "$VMV_VIDEO_FIXTURES/short.mp4"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i sine=frequency=440:sample_rate=48000 \
    -t 1 -c:a aac "$VMV_VIDEO_FIXTURES/audio-only.m4a"

export CARGO_TARGET_DIR="$test_dir/build"
export CARGO_HOME="$test_dir/cargo-home"
timeout 180s cargo test --manifest-path "$test_dir/Cargo.toml" \
    --target x86_64-unknown-linux-gnu "$@" -- --test-threads=1
