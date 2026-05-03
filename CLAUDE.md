# visual-media-viewer

## Overview
High-performance image and media viewer built with Rust + wgpu + egui. Targeting Windows as the primary platform, with GPU-accelerated rendering for fast image display and smooth file navigation.

## Tech Stack
- **Language:** Rust
- **GPU Rendering:** wgpu
- **GUI Framework:** egui + eframe (wgpu backend)
- **Image Decoding:** image crate (JPEG, PNG, GIF, BMP)
- **Async Runtime:** tokio (for preloading)
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
    main.rs          # Application entry point, eframe setup
  scripts/
    build.sh         # Build script (wraps docker compose)
  Cargo.toml         # Rust dependencies
  Dockerfile.build   # Cross-compilation image (rust + mingw-w64)
  docker-compose.yml # Build service definition
  TASK.md            # Project roadmap and task tracking
  CLAUDE.md          # This file
```

## Development
- All builds run inside Docker (do not install Rust in WSL2)
- Target: x86_64-pc-windows-gnu (Windows .exe)
- Build cache persists in ./target-docker/
