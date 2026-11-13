#!/bin/bash
set -euo pipefail

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

DIST_DIR="$PROJECT_DIR/dist/$PROFILE"
mkdir -p "$DIST_DIR"
cp "$EXE_PATH" "$DIST_DIR/"

for name in VisualMediaViewerThumbnail.dll avcodec-61.dll avdevice-61.dll \
    avfilter-10.dll avformat-61.dll avutil-59.dll swresample-5.dll swscale-8.dll; do
    cp "$OUT_DIR/$name" "$DIST_DIR/"
done

for name in LGPL-2.1.txt THIRD_PARTY_LICENSES.txt; do
    cp "$PROJECT_DIR/licenses/$name" "$DIST_DIR/"
done
cp "$PROJECT_DIR/LICENSE" "$DIST_DIR/LICENSE.txt"

echo ""
echo "=== Build successful [$PROFILE] ==="
echo "Output:"
ls -lh "$DIST_DIR/VisualMediaViewer.exe"
