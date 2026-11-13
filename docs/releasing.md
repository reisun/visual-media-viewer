# Release preparation

## Layout

- `scripts/windows-diagnostics/`: development-only Windows diagnostics and smoke
  tests. These are never copied into a normal build or official release.
- `dist/release/`: release inputs: the executable, thumbnail provider, seven
  FFmpeg DLLs and three licenses, exactly 12 files.
- `dist/debug/`: debug build inputs, separate from release inputs.
- `dist/releases/`: versioned official ZIP archives.
- `dist/verification/`: local preview archives and previous verification builds.
- `dist/backups/`: local backup executables.
- `dist/release-notes/`: historical local release notes. New release notes live
  in `docs/release-notes-vVERSION.md` so they are tracked and reviewed.

All `dist` contents are ignored by Git. Source diagnostic tools remain tracked.
Old previews and backups are kept outside the release inputs, not deleted.

## Build And Package

Update the package version in both Cargo.toml files and both Cargo.lock entries.
Run these commands from the repository root:

```bash
./scripts/build.sh release
docker run --rm --user "$(id -u):$(id -g)" \
  -e PYTHONDONTWRITEBYTECODE=1 -v "$PWD:/app" -w /app python:3.12-slim \
  python -m unittest discover -s scripts -p test_package_release.py
docker run --rm --user "$(id -u):$(id -g)" \
  -e PYTHONDONTWRITEBYTECODE=1 -v "$PWD:/app" -w /app python:3.12-slim \
  python scripts/package-release.py
```

Packaging requires matching application/provider versions. It rejects extra or
missing files, directories, symbolic links and empty files in `dist/release`.
If rejected, inspect and move unrelated inputs to the appropriate verification
or backup directory; do not bypass validation. The ZIP is freshly generated,
checked for its exact file list and CRC integrity, then atomically replaces the
same-version archive. It never appends to an earlier preview ZIP.

## Publish

Merge the release PR first. Create the GitHub release with a new `vVERSION` tag
targeting that exact merged commit, upload only the verified ZIP, and use the
tracked release notes. Verify the published tag/commit, non-draft release state,
asset name/size and the downloaded asset's hash against the local ZIP.
Diagnostic scripts and preview ZIPs must not be attached to the release.
