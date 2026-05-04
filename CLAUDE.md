# visual-media-viewer

## Overview
High-performance image and media viewer built with Rust + wgpu + egui. Targeting Windows as the primary platform, with GPU-accelerated rendering for fast image display and smooth file navigation.

## Tech Stack
- **Language:** Rust
- **GPU Rendering:** wgpu
- **GUI Framework:** egui + eframe (wgpu backend)
- **Image Decoding:** image crate (JPEG, PNG, GIF, BMP, WebP, TIFF) + Windows WIC (HEIC/HEIF)
- **Video:** FFmpeg (LGPL DLL dynamic link) — decode, audio resample
- **Audio:** cpal crate — platform audio output
- **Background Decoding:** std::thread + std::sync::mpsc (preloading, video decode)
- **IPC:** TCP localhost (multi-instance prevention)
- **Logging:** log + env_logger

## Build
Docker-based cross-compilation (Linux -> Windows .exe):

```bash
./scripts/build.sh
# or
docker compose run --rm build
```

Output: `target-docker/x86_64-pc-windows-gnu/release/visual-media-viewer.exe`

## Directory Structure
```
visual-media-viewer/
  src/
    main.rs          # Entry point, CLI arg parsing, eframe setup, font config
    viewer.rs        # ViewerApp: image/video display, zoom/pan, keyboard, UI
    file_list.rs     # FileList: directory scan, sorted navigation, extension filter
    cache.rs         # ImageCache: byte-based LRU cache (512MB) with background preload
    video_player.rs  # VideoPlayer: FFmpeg decode, audio sync, 2-thread architecture
    image_decode.rs  # DecodedImage: decode, mipmap, texture upload
    ipc.rs           # IPC: TCP listener/sender for multi-instance prevention
    settings.rs      # Settings: window size, volume, persistent JSON config
    wic_decoder.rs   # Windows WIC fallback decoder (HEIC/HEIF)
  scripts/
    build.sh         # Build script (wraps docker compose)
  assets/
    icon.png         # Application icon
  Cargo.toml         # Rust dependencies
  Dockerfile.build   # Cross-compilation image (rust + mingw-w64 + FFmpeg)
  docker-compose.yml # Build service definition
  TASK.md            # Project roadmap and task tracking
  CLAUDE.md          # This file
```

## Keyboard Shortcuts
- **Arrow Left/Right:** Navigate previous/next file (loops) / Video: seek ±10s
- **Arrow Up/Down:** Navigate to previous/next sibling folder
- **PgUp:** Navigate to parent directory
- **PgDn:** Navigate into first child subdirectory with images
- **Space:** Play/pause video
- **R:** Rotate clockwise 90 degrees
- **Shift+R:** Rotate counter-clockwise 90 degrees
- **S:** Toggle slideshow
- **+/=:** Increase slideshow interval (+0.1s, max 30s)
- **-:** Decrease slideshow interval (-0.1s, min 1s)
- **Mouse wheel:** Zoom in/out (image) / Volume adjust (video)
- **Right-click drag:** Zoom by vertical movement
- **Double-click:** Reset view to fit

## Title Bar
- Custom title bar (no OS decorations)
- Title format: `親フォルダ/ファイル名 (現在 / 画像件数)` + slideshow indicator `<自動: X.Xs>`
- Drag to move window, double-click to maximize/restore
- Right-click to open menu (リスト / 表示)

## Development
- All builds run inside Docker (do not install Rust in WSL2)
- Target: x86_64-pc-windows-gnu (Windows .exe)
- Build cache persists in ./target-docker/
