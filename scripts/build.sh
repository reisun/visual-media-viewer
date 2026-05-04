#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

PROFILE="${1:-debug}"

echo "=== Building visual-media-viewer [$PROFILE] (Windows x86_64) ==="
docker compose run --rm build bash scripts/docker-build.sh "$PROFILE"

if [ "$PROFILE" = "check" ]; then
    echo ""
    echo "=== Check passed ==="
    exit 0
fi

if [ "$PROFILE" = "release" ]; then
    OUT_DIR="target-docker/x86_64-pc-windows-gnu/release"
else
    OUT_DIR="target-docker/x86_64-pc-windows-gnu/debug"
fi

EXE_PATH="$OUT_DIR/VisualMediaViewer.exe"

if [ ! -f "$EXE_PATH" ]; then
    echo ""
    echo "=== Build completed but .exe not found at expected path ==="
    echo "Searching for .exe files..."
    find target-docker -name "*.exe" -type f 2>/dev/null || echo "No .exe files found"
    exit 1
fi

DIST_DIR="$PROJECT_DIR/dist"
mkdir -p "$DIST_DIR"
cp "$EXE_PATH" "$DIST_DIR/"

DLL_COUNT=$(find "$OUT_DIR" -maxdepth 1 -name "*.dll" -type f 2>/dev/null | wc -l)
if [ "$DLL_COUNT" -gt 0 ]; then
    cp "$OUT_DIR"/*.dll "$DIST_DIR/"
    echo "Copied $DLL_COUNT FFmpeg DLL(s) to dist/"
else
    echo "WARNING: No FFmpeg DLLs found in $OUT_DIR"
fi

cp "$PROJECT_DIR/licenses/"* "$DIST_DIR/"

echo ""
echo "=== Build successful [$PROFILE] ==="
echo "Output:"
ls -lh "$DIST_DIR/VisualMediaViewer.exe"
