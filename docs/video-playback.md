# Video playback and startup

## Frame upload

The player assigns a revision whenever it selects a new current frame. Repeated
polls, including paused redraws, retain that revision. The viewer copies pixels
and uploads the texture only when the revision differs from the last uploaded
revision. Presentation timestamps are not used as frame identity: distinct
frames can have identical timestamps, and seeking can revisit a timestamp.

Revisions survive seeks within one player. Destroying the video texture resets
the viewer's upload tracking, including when changing files. Polling and the
existing presentation clock continue even when no upload is needed.

The buffering indicator is evaluated after polling and only while playback is
playing and awaiting its first frame. Finishing without a decoded frame clears
that wait state. Finished playback shows a stop square instead of a spinner;
seeking past the end when no other file is available stops playback. Space or a
valid seek can restart playback through the existing controls.

## Initial open

Opening a video prepares the input and threaded video decoder once. Metadata
and the initial playback session use that same preparation instead of opening
the input and video decoder twice. Stream selection, probe limits, decoder
thread settings, audio setup, and delayed scaler initialization are preserved.

## Seek

Seeking still stops audio output, releases the frame receiver, joins both
workers, clears displayed and buffered frames, and recreates video/audio
decoders, resamplers, queues, and clocks. The change is limited to recovering
the input context from the finished demux worker and seeking it to the new
position, avoiding another file open and stream analysis where reuse succeeds.
Seeking to zero also explicitly repositions reused input.

If input recovery or preparation for reuse fails, playback opens a fresh input.
If seeking the freshly opened input also fails, playback stops rather than
silently playing from an incorrect position.
Set `VMV_DISABLE_INPUT_REUSE=1` before launching the application to retain the
full input-reopen path for comparison or compatibility. A successful FFmpeg
seek alone does not guarantee compatibility with every file and device.

The existing 8-frame/2000-ms prebuffer, audio master clock and wall-clock
fallback, global packet/audio byte limit, and nonblocking demux sends remain
unchanged. Scaler setup still waits for a decoded frame, preserving the MOV
pixel-format fix.

## Validation and limitations

Use `docker compose -f scripts/video-tests/compose.yml run --build --rm video-tests`
for the isolated native Docker video tests.
Use the existing Windows Docker build for target compilation and release
output. Native tests exercise production player code with generated video
fixtures; they do not validate Windows audio devices or GPU presentation.

Generated very short clips and seeks immediately before EOF can finish without
producing a displayed frame in both the reuse and full-reopen paths. Regression
tests compare these outcomes explicitly. The existing decoder EOF-drain logic
is unchanged by this optimization; fixing that behavior is separate work.

On Windows, compare the previous stable executable with the new executable
using files that previously exposed playback failures. Cover initial open,
rapid forward/backward seeks, zero and end positions, paused seeks, EOF restart,
image/video transitions, MOV, video without audio, and long playback. Check
audio/video drift, missing or stale frames, shutdown, and memory growth.

Decoder and audio-device reuse remain future work. Before extending reuse,
define and test complete reset of codec buffers, resampler delay, pending
packets/frames, audio callback data, clocks, and session identities. A measured
benefit and regression coverage on previously problematic Windows files are
required to justify that broader change.
