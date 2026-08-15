# Navigation and Title Behavior

## Scope

This document describes the confirmed keyboard, slideshow, and title-bar behavior for Visual Media Viewer.

## File navigation

- `←` / `→`
  - Image: previous / next file
  - Video: seek `-10s` / `+10s`
- `PgUp` / `PgDn`
  - Image: move `-5` / `+5` files
  - When the move lands beyond an edge, clamp to the edge on that press
  - Only wrap to the opposite edge when the same key is pressed again while already at the edge
  - Video: keep `-5min` / `+5min`
- `↑` / `↓`
  - Move to previous / next image directory using existing directory traversal
- `Shift+↑` / `Shift+↓`
  - Move to the previous / next sibling branch of the current directory's parent
  - No loop

## Slideshow

- Persist only the interval
- Always start with slideshow OFF at launch
- `S`: force ON
- `Shift+S`: force OFF
- `S+D`: interval `+0.1s`
- `S+F`: interval `-0.1s`
- `+` / `-`: keep interval adjustment shortcuts
- Successful manual file transitions reset the slideshow timer
- While a video is active, ignore the interval timer
- Advance exactly once when the current video reaches `PlaybackState::Finished`, including abnormal finish paths

## Title bar

- `title_root` starts as the current parent directory
- On directory changes, update `title_root` to the cumulative least common ancestor with the new current parent
- If there is no usable common root, reset `title_root` to the new current parent
- Display format:
  - `root-name/relative-child/file-name`
  - then the existing position and slideshow/video suffixes
- `N` resets `title_root` to the current parent

## Diagnostics and activation

- Diagnostics hotkey moved from `D` to `F12`
- On Windows IPC open requests, restore and re-activate the existing window before focusing the new file
