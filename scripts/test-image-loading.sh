#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."

# Reuse the application's dependency versions; keep generated files in target.
test_dir="$PWD/target/image-loading-tests"
mkdir -p "$test_dir"
cp scripts/image-loading-tests/Cargo.toml "$test_dir/Cargo.toml"
cp Cargo.lock "$test_dir/Cargo.lock"
cp scripts/image-loading-tests/lib.rs "$test_dir/lib.rs"

# The build image defaults to Windows libjpeg-turbo. Native tests build their own.
unset TURBOJPEG_LIB_DIR TURBOJPEG_INCLUDE_DIR TURBOJPEG_STATIC
export CARGO_TARGET_DIR="$test_dir/build"
cargo test --manifest-path "$test_dir/Cargo.toml" --target x86_64-unknown-linux-gnu "$@"
