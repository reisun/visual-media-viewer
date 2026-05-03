# visual-media-viewer

## Overview
High-performance image and media viewer built with Rust + wgpu + egui. Targeting Windows as the primary platform, with GPU-accelerated rendering for fast image display and smooth file navigation.

## Tech Stack
- **Language:** Rust
- **GPU Rendering:** wgpu
- **GUI Framework:** egui + eframe (wgpu backend)
- **Image Decoding:** image crate (JPEG, PNG, GIF, BMP, WebP, TIFF)
- **Background Decoding:** std::thread + std::sync::mpsc (preloading)
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
    main.rs          # Application entry point, CLI arg parsing, eframe setup
    viewer.rs        # ViewerApp: image display, zoom/pan, keyboard input
    file_list.rs     # FileList: directory scan, sorted navigation, extension filter
    cache.rs         # ImageCache: LRU cache with background preload (std::thread)
  scripts/
    build.sh         # Build script (wraps docker compose)
  Cargo.toml         # Rust dependencies
  Dockerfile.build   # Cross-compilation image (rust + mingw-w64)
  docker-compose.yml # Build service definition
  TASK.md            # Project roadmap and task tracking
  CLAUDE.md          # This file
```

## Keyboard Shortcuts
- **Arrow Left/Right:** Navigate previous/next image (loops)
- **R:** Rotate clockwise 90 degrees
- **Shift+R:** Rotate counter-clockwise 90 degrees
- **S:** Toggle slideshow
- **+/=:** Increase slideshow interval (+1s, max 30s)
- **-:** Decrease slideshow interval (-1s, min 1s)
- **I:** Toggle property info overlay
- **Mouse wheel:** Zoom in/out
- **Right-click drag:** Zoom by vertical movement
- **Right-click (no drag):** Context menu
- **Double-click:** Reset view to fit

## Development
- All builds run inside Docker (do not install Rust in WSL2)
- Target: x86_64-pc-windows-gnu (Windows .exe)
- Build cache persists in ./target-docker/
