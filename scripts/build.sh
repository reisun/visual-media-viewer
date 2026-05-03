#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

echo "=== Building visual-media-viewer (Windows x86_64) ==="
docker compose run --rm build

EXE_PATH="target-docker/x86_64-pc-windows-gnu/release/visual-media-viewer.exe"

if [ ! -f "$EXE_PATH" ]; then
    echo ""
    echo "=== Build completed but .exe not found at expected path ==="
    echo "Searching for .exe files..."
    find target-docker -name "*.exe" -type f 2>/dev/null || echo "No .exe files found"
    exit 1
fi

DIST_DIR="$PROJECT_DIR/dist"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"
cp "$EXE_PATH" "$DIST_DIR/"

echo ""
echo "=== Build successful ==="
echo "Release files:"
ls -lh "$DIST_DIR/"
