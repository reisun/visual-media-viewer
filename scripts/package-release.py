#!/usr/bin/env python3
"""Package only verified release inputs, never a preview or diagnostic directory."""

import argparse
import os
from pathlib import Path
import re
import tempfile
import tomllib
import zipfile


RELEASE_FILES = (
    "VisualMediaViewer.exe",
    "VisualMediaViewerThumbnail.dll",
    "avcodec-61.dll",
    "avdevice-61.dll",
    "avfilter-10.dll",
    "avformat-61.dll",
    "avutil-59.dll",
    "swresample-5.dll",
    "swscale-8.dll",
    "LICENSE.txt",
    "LGPL-2.1.txt",
    "THIRD_PARTY_LICENSES.txt",
)


def release_version(root: Path) -> str:
    versions = []
    for manifest in (root / "Cargo.toml", root / "shell-thumbnail/Cargo.toml"):
        with manifest.open("rb") as file:
            versions.append(tomllib.load(file)["package"]["version"])
    version = versions[0]
    if version != versions[1]:
        raise ValueError("Application and thumbnail provider versions do not match")
    if not isinstance(version, str) or not re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?",
        version,
    ):
        raise ValueError("Invalid release version")
    return version


def validate_source(source: Path) -> None:
    if source.is_symlink() or not source.is_dir():
        raise ValueError("Release source must be a real directory")
    entries = {entry.name: entry for entry in source.iterdir()}
    expected = set(RELEASE_FILES)
    missing = expected - entries.keys()
    unexpected = entries.keys() - expected
    if missing or unexpected:
        raise ValueError(
            f"Release contents differ: missing={sorted(missing)}, "
            f"unexpected={sorted(unexpected)}"
        )
    for name in RELEASE_FILES:
        entry = entries[name]
        if entry.is_symlink() or not entry.is_file() or entry.stat().st_size == 0:
            raise ValueError(f"Release input must be a nonempty regular file: {name}")


def package_release(
    root: Path, source: Path | None = None, output_dir: Path | None = None
) -> Path:
    source = source if source is not None else root / "dist/release"
    output_dir = output_dir if output_dir is not None else root / "dist/releases"
    version = release_version(root)
    validate_source(source)
    if output_dir.resolve().is_relative_to(source.resolve()):
        raise ValueError("Archive output must be outside the release source")
    output_dir.mkdir(parents=True, exist_ok=True)
    destination = output_dir / f"VisualMediaViewer-v{version}.zip"
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            prefix=".release-", suffix=".zip", dir=output_dir, delete=False
        ) as file:
            temporary = Path(file.name)
        with zipfile.ZipFile(
            temporary, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
        ) as archive:
            for name in RELEASE_FILES:
                archive.write(source / name, arcname=name)
        with zipfile.ZipFile(temporary) as archive:
            if archive.namelist() != list(RELEASE_FILES) or archive.testzip() is not None:
                raise ValueError("Release archive verification failed")
        os.replace(temporary, destination)
        return destination
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    try:
        print(package_release(args.root, args.source, args.output_dir))
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Release packaging failed: {error}\n")


if __name__ == "__main__":
    main()
