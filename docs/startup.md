# Startup

Settings are loaded once before window creation and passed to the viewer.
Window geometry, sorting, image fit, rotation and playback preferences use this
same snapshot.

The command-line file is queued while the first native UI frame draws the title
bar and loading overlay. `raw_input_hook` counts native frames, not egui layout
passes. File opening starts during the second native frame, using the actual
viewport dimensions and DPI for the first JPEG preview. A newer IPC or dropped
file cancels the pending command-line file, including when the newer path fails
to open. Launching without a file does not enter this loading state.

This moves folder enumeration and synchronous video preparation after the first
UI frame; it does not make those operations asynchronous. The window may still
pause while a slow directory or video is opened. Video decoder, audio and
buffering behavior is unchanged.

Folder lists scan and sort once using the saved file order. Modified-date sort
captures each file timestamp once per scan and uses stable sorting so equal
timestamps retain enumeration order. Explicit sort changes still rescan the
directory and preserve the selected path. Folder-navigation sorting preferences
remain separate from file sorting.

## Validation

Run `docker compose run --rm build bash scripts/test-image-loading.sh` for file
ordering, selection preservation, equal timestamps, deferred startup and
physical-pixel preview sizing. Run `./scripts/build.sh check` for Windows target
compilation. Native Docker tests do not validate Windows window presentation.

On Windows, compare process launch to window display and to first media display
separately, with cold and warm filesystem caches. Check large folders with name
and modified-date sorting in both directions, small/high-DPI/maximized windows,
original-size mode, corrupt or missing initial files, image/video launches,
rapid IPC file changes and file drops during startup. Verify that resizing and
zooming still upgrade JPEG previews and that video audio/seek/EOF behavior is
unchanged. No fixed startup speedup is assumed without Windows measurements.
