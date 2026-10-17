"""Structural packager regressions; no plugin executable is run."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("package_plugin", Path(__file__).with_name("package-plugin.py"))
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class PackageTests(unittest.TestCase):
    def prepare(self, directory, native=True, filters=True):
        root = Path(directory) / "prepared"
        root.mkdir()
        metadata = {"packageVersion": 2, "module": "plugin.pcext"}
        (root / "plugin.pcext").write_bytes(b"\0asm\x01\0\0\0")
        (root / "LICENSE").write_text("Example license", encoding="utf-8")
        (root / "THIRD-PARTY-NOTICES.md").write_text("Example notices", encoding="utf-8")
        if native:
            metadata["native"] = {"service": "plugin.processImage", "platform": "windows-x86_64", "entry": "worker.exe"}
            (root / "worker.exe").write_bytes(b"structural fixture, never executed")
        if filters:
            metadata["filters"] = [{"module": "filters/gain.wasm"}]
            (root / "filters").mkdir()
            (root / "filters/gain.wasm").write_bytes(b"\0asm\x01\0\0\0")
        (root / "package.json").write_text(json.dumps(metadata), encoding="utf-8")
        return root

    def test_deterministic_complete_archives_and_unix_executable_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.prepare(directory)
            first = packager.package(root, Path(directory) / "first.pcplugin")
            second = packager.package(root, Path(directory) / "second.pcplugin")
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                self.assertIsNone(archive.testzip())
                self.assertTrue(archive.getinfo("worker.exe").external_attr >> 16 & 0o111)
                self.assertEqual(archive.read("plugin.pcext"), b"\0asm\x01\0\0\0")

    def test_panel_filter_native_and_combined_packages(self):
        for native, filters in [(False, False), (False, True), (True, False), (True, True)]:
            with self.subTest(native=native, filters=filters), tempfile.TemporaryDirectory() as directory:
                root = self.prepare(directory, native, filters)
                packager.package(root, Path(directory) / "result.pcplugin")

    def test_unsafe_names_are_rejected(self):
        for name in ["../escape", "/absolute", "C:/path", "back\\slash", "a//b", "a/./b", "CON.txt", "foo/NUL", "file.", ".photocraft-install.json"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                packager.safe_path(name)

    def test_missing_files_wrong_modules_and_unknown_metadata_fail(self):
        for change in ("missing", "module", "metadata", "native", "filters"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = self.prepare(directory)
                metadata = json.loads((root / "package.json").read_text(encoding="utf-8"))
                if change == "missing":
                    (root / "LICENSE").unlink()
                elif change == "module":
                    (root / "plugin.pcext").write_bytes(b"not wasm")
                elif change == "metadata":
                    metadata["installScript"] = "something.exe"
                elif change == "native":
                    metadata["native"]["entry"] = "../worker.exe"
                else:
                    metadata["filters"] *= 17
                (root / "package.json").write_text(json.dumps(metadata), encoding="utf-8")
                with self.assertRaises(ValueError):
                    packager.package(root, Path(directory) / "bad.pcplugin")

    def test_archive_cannot_be_written_into_its_own_input_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.prepare(directory)
            with self.assertRaises(ValueError):
                packager.package(root, root / "bad.pcplugin")


if __name__ == "__main__":
    unittest.main()
