import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile


spec = importlib.util.spec_from_file_location(
    "package_release", Path(__file__).with_name("package-release.py")
)
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


class PackageReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "shell-thumbnail").mkdir()
        for manifest in ("Cargo.toml", "shell-thumbnail/Cargo.toml"):
            (self.root / manifest).write_text('[package]\nversion = "0.6.0"\n')
        self.source = self.root / "dist/release"
        self.source.mkdir(parents=True)
        for name in packaging.RELEASE_FILES:
            (self.source / name).write_bytes(f"fixture {name}".encode())

    def test_archive_contains_only_exact_release_files(self):
        result = packaging.package_release(self.root)
        self.assertEqual(result.name, "VisualMediaViewer-v0.6.0.zip")
        with zipfile.ZipFile(result) as archive:
            self.assertEqual(archive.namelist(), list(packaging.RELEASE_FILES))
            for name in packaging.RELEASE_FILES:
                self.assertEqual(archive.read(name), (self.source / name).read_bytes())

    def test_missing_file_is_rejected(self):
        (self.source / "VisualMediaViewerThumbnail.dll").unlink()
        with self.assertRaisesRegex(ValueError, "missing="):
            packaging.package_release(self.root)

    def test_extra_diagnostic_or_preview_is_rejected(self):
        for name in ("diagnose-windows-thumbnails.cmd", "preview.zip", "backup.exe"):
            with self.subTest(name=name):
                extra = self.source / name
                extra.write_bytes(b"not for release")
                with self.assertRaisesRegex(ValueError, "unexpected="):
                    packaging.package_release(self.root)
                extra.unlink()

    def test_nested_directory_is_rejected(self):
        (self.source / "diagnostics").mkdir()
        with self.assertRaisesRegex(ValueError, "unexpected="):
            packaging.package_release(self.root)

    def test_directory_with_expected_filename_is_rejected(self):
        path = self.source / "LICENSE.txt"
        path.unlink()
        path.mkdir()
        with self.assertRaisesRegex(ValueError, "regular file"):
            packaging.package_release(self.root)

    def test_symlink_is_rejected(self):
        path = self.source / "LICENSE.txt"
        path.unlink()
        path.symlink_to(self.root / "Cargo.toml")
        with self.assertRaisesRegex(ValueError, "regular file"):
            packaging.package_release(self.root)

    def test_symlink_source_is_rejected(self):
        link = self.root / "linked-source"
        link.symlink_to(self.source, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "real directory"):
            packaging.package_release(self.root, source=link)

    def test_empty_file_is_rejected(self):
        (self.source / "LICENSE.txt").write_bytes(b"")
        with self.assertRaisesRegex(ValueError, "nonempty"):
            packaging.package_release(self.root)

    def test_case_sensitive_allowlist(self):
        (self.source / "LICENSE.txt").rename(self.source / "license.txt")
        with self.assertRaisesRegex(ValueError, "Release contents differ"):
            packaging.package_release(self.root)

    def test_version_mismatch_is_rejected(self):
        (self.root / "shell-thumbnail/Cargo.toml").write_text(
            '[package]\nversion = "0.5.0"\n'
        )
        with self.assertRaisesRegex(ValueError, "versions do not match"):
            packaging.package_release(self.root)

    def test_invalid_version_is_rejected(self):
        for manifest in ("Cargo.toml", "shell-thumbnail/Cargo.toml"):
            (self.root / manifest).write_text('[package]\nversion = "../outside"\n')
        with self.assertRaisesRegex(ValueError, "Invalid release version"):
            packaging.package_release(self.root)

    def test_existing_archive_is_replaced_not_appended(self):
        output = self.root / "dist/releases"
        output.mkdir()
        previous = output / "VisualMediaViewer-v0.6.0.zip"
        with zipfile.ZipFile(previous, "w") as archive:
            archive.writestr("diagnose-windows-thumbnails.cmd", "stale")
        result = packaging.package_release(self.root)
        with zipfile.ZipFile(result) as archive:
            self.assertEqual(archive.namelist(), list(packaging.RELEASE_FILES))
        self.assertEqual(list(output.iterdir()), [result])

    def test_invalid_stage_preserves_previous_archive(self):
        result = packaging.package_release(self.root)
        before = result.read_bytes()
        (self.source / "debug.txt").write_bytes(b"not for release")
        with self.assertRaises(ValueError):
            packaging.package_release(self.root)
        self.assertEqual(result.read_bytes(), before)

    def test_output_inside_source_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "outside the release source"):
            packaging.package_release(self.root, output_dir=self.source)

    def test_explicit_source_and_output(self):
        source = self.root / "custom-input"
        self.source.rename(source)
        output = self.root / "custom-output"
        result = packaging.package_release(self.root, source=source, output_dir=output)
        self.assertEqual(result.parent, output)
        self.assertTrue(result.is_file())


if __name__ == "__main__":
    unittest.main()
