# Windows integration

Open **Windowsとの連携...** from the title-bar context menu. No registration or
registry inspection happens during normal startup. Registration operations and
state inspection run on a background thread; playback does not depend on them.

## File associations

**関連付け候補に登録** independently registers this app as a candidate in Windows
Default Apps and Open With for the same 21 image/video extensions as the viewer.
**既定のアプリを設定...** registers a missing candidate and opens Windows Settings.
The user makes the default selection there. The app never writes `UserChoice`
or forces a default. Actual default status is resolved through
`AssocQueryStringW`, not a saved checkbox preference.

Turning candidate registration off removes only owned candidate entries. A
ProgID command still used by an effective default or referenced by `UserChoice`
is retained so double-clicking a file keeps working. Change its default through
Windows Settings. Thumbnail registration is not changed by either operation.

## Explorer thumbnails

**サムネイル対応を登録** independently registers the thumbnail COM provider for
the supported extensions in the current user's
`Software\Classes\.ext\shellex\{E357FCCD-A995-4576-B01F-234630154E96}`.
Its class is `{D4F66B4E-5762-4BA9-9633-138915FD01A7}` and uses the Apartment
threading model. The existing user override is backed up with its original
registry type and bytes, including the difference between absent and empty.
Disabling restores that override only if this app's handler is still registered;
a later change made by another app is preserved. Machine-wide settings are not
modified. Changes notify the shell with `SHChangeNotify(SHCNE_ASSOCCHANGED)`.

The screen reports both explicit registration and the effective handler resolved
by Windows. Other higher-priority associations, policies, codecs, Explorer's
thumbnail settings and cached results can affect actual display. The app does
not delete the global thumbnail cache. Partial or damaged registration offers
repair and removal. A valid older installation remains checked and offers an
update action instead of pretending it is unregistered.

Registry changes are journaled and restored on ordinary operation failure.
Rollback failure is logged, and a process interruption may still leave a partial
state that must be repaired or removed from this screen.

## Portable deployment

The optional features copy their required files into versioned directories below
`%LOCALAPPDATA%\VisualMediaViewer\Shell`. Default-app commands and COM registration
point there, so moving the original portable folder does not break them. Identical
deployments are byte-compared and reused when toggling or repairing. Failed new
copies remove only their own known files. Previously deployed versions remain
because Explorer can still hold their DLL and invoke their matching worker.
They must not be deleted while Explorer/worker processes are using them.

The normal portable app requires no installation. Optional registration persists
after closing or deleting the portable app; disable the desired feature before
discarding it. Disabling does not delete deployment directories or a Windows
default choice. License files accompany deployed FFmpeg libraries.

## Thumbnail architecture

`shell-thumbnail` builds `VisualMediaViewerThumbnail.dll`, a standalone COM server
implementing `IClassFactory`, `IInitializeWithStream` and `IThumbnailProvider`.
It imports only Windows system libraries, retaining Windows' default thumbnail
process isolation. It does not load FFmpeg into Explorer. Decoding runs in a
hidden, same-directory `VisualMediaViewer.exe --shell-thumbnail-worker --size N`
process before logging, IPC, GUI, audio or GPU initialization.

The provider services bounded read/seek/size requests against the supplied
`IStream` on the COM apartment thread. This avoids copying entire large videos
or relying on file paths for virtual items. The worker returns straight RGBA;
the provider creates a top-down, premultiplied BGRA HBITMAP for Windows. The
original stream position is restored when seek is available. References and
server locks control `DllCanUnloadNow`.

JPEG uses reduced-resolution TurboJPEG decoding. Other supported images use
the image decoder; HEIC/HEIF uses installed Windows WIC codecs, with scaling
before output conversion. Video uses a separate FFmpeg custom-AVIO decoder,
including delayed-frame draining. Only supported container demuxers are allowed,
external protocols are disabled, and no playback state is reused.

Results are bounded to 1024 pixels per side. Images have a 128 MiB input/decoded
allocation limit; video stream reads are capped at 256 MiB cumulatively, with
frame dimensions/pixels and individual FFmpeg allocations bounded. The worker
has an eight-second I/O budget and the provider kills the child after ten seconds.
This timeout cannot cancel an OS-level `IStream` call already blocked on a network
or cloud source. Oversized, malformed, unsupported or slow media falls back to
Explorer's normal icon/error behavior. The deadline and limits can prevent a
thumbnail for some files the full viewer can open.

## Build and verification

`./scripts/build.sh check` checks both Windows targets. `./scripts/build.sh release`
builds and copies the exe, provider DLL, FFmpeg DLLs and licenses into `dist`.
Keep both Cargo.lock files. Future version changes must update the root and
`shell-thumbnail` package versions together.

Native tests:

```bash
docker compose run --rm build bash scripts/test-image-loading.sh
docker compose run --rm build cargo test --locked --manifest-path scripts/shell-tests/Cargo.toml --target x86_64-unknown-linux-gnu --target-dir /app/target/shell-tests
docker compose -f scripts/thumbnail-tests/compose.yml run --build --rm thumbnail-tests
```

These cover restoration, conflicts, rollback, deployment comparison, layout in
small/large windows, real image/video thumbnail decoding, EOF, RPC boundaries
and resource limits. Native tests do not activate Windows COM or alter the
Windows registry.

On Windows, run `powershell -File scripts/test-windows-shell.ps1` for a direct COM
smoke test without registry changes. Then manually check candidate and thumbnail
registration independently, Windows default selection and return-to-app refresh,
foreign-handler restoration/conflict, interrupted registration repair, update
without losing the actual ON state, and moving the original portable folder.
Check Explorer thumbnails for JPEG/transparent PNG/WebP/TIFF/HEIC, audio/video
MP4, B-frame MOV, MKV, large movies, corrupt files and rapid folder navigation.
Verify playback and seeking remain stable while registration work runs.
