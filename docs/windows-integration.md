# Windows integration

Open **Windowsとの連携...** from the title-bar context menu. No registration or
registry inspection happens during normal startup. Registration operations and
state inspection run on a background thread; playback does not depend on them.

## Application icon

The executable resource and runtime window icon share `assets/icon.ico`.
Windows taskbar/Alt+Tab also receive an explicit `WM_SETICON(ICON_BIG)` using
the executable's shared icon resource; winit's ordinary window icon only sets
`ICON_SMALL`. The shared Windows icon is not destroyed by the application.
Resource compilation errors fail the build rather than silently shipping an
executable without its icon. After replacing an icon, an existing pinned
shortcut may retain a cached image; unpin/re-pin that shortcut if necessary.

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
threading model.
The provider and class factory explicitly disable automatic COM agility, matching
the Apartment registration. A retained `IStream` must stay in its COM apartment;
the implementation must not advertise that it can be called directly from an
arbitrary apartment. Windows uses standard marshaling instead.

The existing user override is backed up with its original registry type and bytes,
including the difference between absent and empty.
Disabling restores that override only if this app's handler is still registered;
a later change made by another app is preserved. Machine-wide settings are not
modified. Changes notify the shell with `SHChangeNotify(SHCNE_ASSOCCHANGED)`.

The screen reports both explicit registration and the effective handler resolved
by Windows. Other higher-priority associations, policies, codecs, Explorer's
thumbnail settings and cached results can affect actual display. The app does
not delete the global thumbnail cache. Partial or damaged registration offers
repair and removal. A valid older installation remains checked and offers an
update action instead of pretending it is unregistered.
The update check compares the installed executable/provider with the current
portable files, including changed builds with the same version number. After
extracting an updated distribution, use **サムネイル部品を更新** here; replacing
the portable files alone does not replace the registered copy. Explorer can
retain the previous DLL until its thumbnail host exits. If it still uses old
components after updating, close Explorer windows and restart Explorer or sign
out and back in. No global cache deletion or forced process termination occurs.

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
process isolation. It does not load FFmpeg into Explorer. Standard Windows image
formats use a direct WIC decoding path, avoiding a new executable process and
stream RPC for each image. Unsupported images and videos use a hidden,
same-directory `VisualMediaViewer.exe --shell-thumbnail-worker --size N` process
before logging, IPC, GUI, audio or GPU initialization.

The provider services bounded read/seek/size requests against the supplied
`IStream` on the COM apartment thread. This avoids copying entire large videos
or relying on file paths for virtual items. The worker returns straight RGBA;
the provider creates a top-down, premultiplied BGRA HBITMAP for Windows. The
original stream position is restored when seek is available. References and
server locks control `DllCanUnloadNow`.

The native path selects the built-in JPEG/PNG/GIF/BMP/TIFF decoder by signature
and scales before RGBA conversion. It does not invoke arbitrary installed codecs.
The worker's JPEG fallback uses reduced-resolution TurboJPEG decoding. Other
supported images use the image decoder; HEIC/HEIF uses installed Windows WIC codecs, with scaling
before output conversion. Video uses a separate FFmpeg custom-AVIO decoder,
including delayed-frame draining. Only supported container demuxers are allowed,
external protocols are disabled, and no playback state is reused.
Image input uses bulk reads rather than many small pipe requests. Video headers
with sufficient decoder metadata can proceed directly to first-frame decoding;
containers requiring stream analysis retain the existing probing fallback.

Results are bounded to 1024 pixels per side. The native path accepts images up
to 128 MiB, 32768 pixels per dimension and 64 megapixels. Worker image decoding
has a 128 MiB input/decoded allocation limit; video stream reads are capped at 256 MiB cumulatively, with
frame dimensions/pixels and individual FFmpeg allocations bounded. The worker
has an eight-second I/O budget and the provider kills the child after ten seconds.
The child timeout does not apply to native WIC decoding in Windows' thumbnail
host and cannot cancel an OS-level `IStream` call blocked on a network or cloud
source. Oversized, malformed, unsupported or slow media falls back to
Explorer's normal icon/error behavior. The deadline and limits can prevent a
thumbnail for some files the full viewer can open.

## Build and verification

`./scripts/build.sh check` checks both Windows targets. `./scripts/build.sh release`
builds and copies only the exe, provider DLL, FFmpeg DLLs and licenses into
`dist/release`. Diagnostic tools remain in `scripts/windows-diagnostics` and
are not part of the official ZIP. Packaging is documented in `releasing.md`.
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

On Windows, run `powershell -File scripts/windows-diagnostics/test-windows-shell.ps1` for a direct COM
smoke test without registry changes. Add `-NativeImagesOnly` to check generated
PNG/JPEG thumbnails with only the provider DLL present, proving the fast path
does not require the worker executable. The script reports generation time and
verifies stream-position restoration as well as pixels and resource cleanup.
Then manually check candidate and thumbnail
registration independently, Windows default selection and return-to-app refresh,
foreign-handler restoration/conflict, interrupted registration repair, update
without losing the actual ON state, and moving the original portable folder.
Check Explorer thumbnails for JPEG/transparent PNG/WebP/TIFF/HEIC, audio/video
MP4, B-frame MOV, MKV, large movies, corrupt files and rapid folder navigation.
Verify playback and seeking remain stable while registration work runs.

## Missing thumbnails

An enabled checkbox confirms registration, not successful DLL activation or
thumbnail extraction. If no thumbnails appear even after updating components,
run `scripts/windows-diagnostics/diagnose-windows-thumbnails.cmd` from the source
checkout. Diagnostic scripts are not included in official releases. By default,
they inspect `dist/release`; to inspect another extracted release, invoke the
PowerShell script with `-DistributionDirectory <folder>`. It records
Windows thumbnail settings, effective extension handlers, component presence,
direct provider tests and a forced Windows Shell extraction of generated test
images. Child probes have time limits so a blocked extraction does not stop the
whole report. The report is `thumbnail-diagnostic.txt` beside the tool.
PNG and JPEG Shell probes run in separate MTA processes, so a stalled PNG does
not prevent JPEG from being checked. This also avoids an unpumped PowerShell STA
being mistaken for a provider failure. Direct provider tests verify that neither
the factory nor the provider advertises automatic `IAgileObject`/`IMarshal`.

The tool does not change registrations or thumbnail settings, erase the global
cache, disable process isolation, or terminate Explorer. Generated test files
are removed after completed tests; a forcibly timed-out child may leave its
small test directory behind. The Shell probe can populate cache entries for its
generated files. Report paths have the current user profile replaced with a
placeholder; review the report before sharing it. A passing direct-DLL test
alone does not prove that Windows can activate the registered handler in its
isolated thumbnail host. Windows runtime results are needed to diagnose that
boundary; Linux decoder tests do not cover it.
