# Image loading

## Selection and background work

Image selection submits a request instead of decoding on the UI thread. The
cache runs at most one decode at a time, prioritizes the latest selected image,
and keeps a bounded queue of nearby files. An already running codec operation
cannot be interrupted; the latest selection runs after that operation finishes.
While work is pending, the UI schedules repaint polling every 16 milliseconds.
When navigating between images, the last displayed texture and its display
position remain visible until the new image is ready. At most one previous
texture is retained, independently of nearby-cache eviction. It is released on
replacement, a load error, or switching to video. The initial load, with no
previous image, still shows a spinner.
The loading spinner is centered over a small translucent backdrop on top of the
retained image. It does not replace or dim the entire image.

Folder changes invalidate results from the previous cache generation. Selecting
a video cancels queued image work. Decode failures belong to the requested path
and are shown only when that path is selected.

## JPEG preview and full resolution

Fit-to-window requests use the longest physical viewport dimension as the JPEG
preview target. The initial request before the first frame uses 2048 pixels.
TurboJPEG chooses an explicit supported DCT scale, rounds decoded dimensions
up, and retains enough pixels for the target when the GPU limit permits it.
Larger windows request an upgraded preview. Zooming beyond the initial fit or
selecting original-size mode requests full resolution, bounded by the GPU's
maximum texture dimension.

The existing preview remains visible while its replacement is decoded. Original
image dimensions are retained independently from texture dimensions, so the
replacement does not change fit, pan, or zoom geometry. Images exceeding GPU
texture limits remain downscaled; tiled original-resolution rendering is not
implemented.

JPEG files use one read on the successful TurboJPEG path, including unscaled
images. Unsupported JPEGs fall back to the existing image/WIC decoders. Other
formats use their existing full decode path on the background worker; they do
not gain JPEG's reduced-resolution decoding.

## Cache and GPU upload

The CPU cache uses its existing byte budget and LRU eviction while pinning the
selected image. A selected image larger than the budget is permitted; background
images that cannot fit alongside it are discarded. The budget does not include
the active decoder's temporary buffers or GPU allocations.

Nearby images are prefetched as previews where supported. Completed replacements
invalidate their previous GPU textures. The selected image has priority for
texture upload. GPU texture upload and mipmap generation still happen through
the UI render state; large full-resolution uploads can still delay a frame.

## Verification

`./scripts/build.sh check` checks the Windows target in Docker. The focused
image-loading tests use
`docker compose run --rm build bash scripts/test-image-loading.sh` and exercise
the real cache and decoder modules without linking the video player. They run
natively inside Docker and do not exercise Windows WIC or GPU display.

For Windows runtime checks, open a large JPEG, rapidly navigate between files
and folders, zoom while the preview is visible, switch to original-size mode,
resize the window, and open a corrupt image. Check PNG/HEIC fallback behavior
and navigation between images and videos. Compare cold first display and warm
navigation separately. `RUST_LOG=debug` includes JPEG read/decode timings; these
are CPU timings and do not measure completion of GPU execution.
