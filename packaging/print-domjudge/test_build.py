#!/usr/bin/env python3
"""Focused package input and dependency-inspection tests (no downloads/builds)."""

import pathlib
import tempfile
import hashlib
import unittest
from unittest.mock import patch

import build
import assets


class PackageInputsTest(unittest.TestCase):
    def test_standalone_build_defaults_to_only_the_wrapper(self):
        version, binary, build_wrapper = build.package_inputs({})
        self.assertTrue(version)
        self.assertEqual(binary, build.REPOSITORY / "target/release/natsume-print-domjudge")
        self.assertTrue(build_wrapper)

    def test_ci_version_and_release_directory_are_preserved_without_build(self):
        for version in ["2.10.3~ci1", "2.10.3-rc.1", "2.10.3"]:
            actual, binary, build_wrapper = build.package_inputs({
                "VERSION": version, "ARCH": "amd64", "RUST_RELEASE_DIR": "/isolated/release"})
            self.assertEqual(actual, version)
            self.assertEqual(binary, pathlib.Path("/isolated/release/natsume-print-domjudge"))
            self.assertFalse(build_wrapper)

    def test_relative_release_directory_is_rooted_at_repository(self):
        _, binary, build_wrapper = build.package_inputs({"RUST_RELEASE_DIR": "target/production/release"})
        self.assertEqual(binary, build.REPOSITORY / "target/production/release/natsume-print-domjudge")
        self.assertFalse(build_wrapper)

    def test_invalid_arch_version_and_empty_prebuilt_path_fail_closed(self):
        for environment in [{"ARCH": "arm64"}, {"ARCH": "x86_64"}, {"ARCH": ""},
                            {"VERSION": ""}, {"VERSION": "2.0\ninjected: value"},
                            {"RUST_RELEASE_DIR": ""}]:
            with self.assertRaises(ValueError):
                build.package_inputs(environment)

    @patch.object(build.subprocess, "check_output", return_value="shlibs:Depends=libc6 (>= 2.39), libgcc-s1 (>= 4.2)\n")
    def test_both_executables_are_inspected_for_shared_libraries(self, inspect):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binaries = [root / "natsume-print-domjudge", root / "typst"]
            for binary in binaries:
                binary.write_bytes(b"test executable")
            dependencies = build.shared_library_dependencies(binaries, root)
        command = inspect.call_args.args[0]
        self.assertEqual(command.count("-e"), 2)
        self.assertTrue(command[3].endswith("/natsume-print-domjudge"))
        self.assertTrue(command[5].endswith("/typst"))
        self.assertEqual(dependencies, "  - libc6 (>= 2.39)\n  - libgcc-s1 (>= 4.2)")


class AssetVerificationTest(unittest.TestCase):
    @patch.object(assets.urllib.request, "urlopen")
    def test_valid_cached_asset_is_verified_without_network(self, download):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "fixed").write_bytes(b"reviewed bytes")
            expected = hashlib.sha256(b"reviewed bytes").hexdigest()
            self.assertEqual(assets.fetch(root, "fixed", "https://example.invalid/fixed", expected), root / "fixed")
            download.assert_not_called()

    @patch.object(assets.urllib.request, "urlopen")
    def test_corrupt_cached_asset_fails_without_silent_replacement(self, download):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "fixed").write_bytes(b"corrupt bytes")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                assets.fetch(root, "fixed", "https://example.invalid/fixed", "0" * 64)
            self.assertEqual((root / "fixed").read_bytes(), b"corrupt bytes")
            download.assert_not_called()


if __name__ == "__main__":
    unittest.main()
