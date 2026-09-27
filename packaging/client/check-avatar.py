#!/usr/bin/env python3
"""Offline tests for the package-time Gravatar input."""
import importlib.util
import json
from pathlib import Path
import stat
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zlib

SPEC = importlib.util.spec_from_file_location(
    "prepare_avatar", Path(__file__).with_name("prepare-avatar.py")
)
AVATAR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AVATAR)
PROFILE = {"avatar_url": "https://0.gravatar.com/avatar/example"}


def png(size=512):
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress((b"\x00" + b"\x80" * size * 3) * size))
            + chunk(b"IEND", b""))


class AvatarPackaging(unittest.TestCase):
    def test_profile_slug_resolves_high_resolution_avatar(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "assets/author-avatar.png"
            image = png()
            with patch.object(AVATAR, "download", side_effect=[json.dumps(PROFILE), image]) as fetch:
                AVATAR.prepare(destination)
            self.assertEqual(fetch.call_args_list[0].args, (AVATAR.PROFILE_URL,))
            self.assertEqual(fetch.call_args_list[1].args,
                             ("https://0.gravatar.com/avatar/example?s=512&d=404",))
            self.assertEqual(destination.read_bytes(), image)
            self.assertEqual(stat.S_IMODE(destination.stat().st_mode), 0o644)

    def test_bad_response_never_replaces_existing_avatar(self):
        for image in (b"", b"<html>error</html>", png(128), png()[:-12]):
            with self.subTest(size=len(image)), tempfile.TemporaryDirectory() as temporary:
                destination = Path(temporary) / "author-avatar.png"
                destination.write_bytes(b"previous image")
                with patch.object(AVATAR, "download", side_effect=[json.dumps(PROFILE), image]):
                    with self.assertRaises(ValueError):
                        AVATAR.prepare(destination)
                self.assertEqual(destination.read_bytes(), b"previous image")

    def test_download_failure_stops_packaging(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "author-avatar.png"
            with patch.object(AVATAR, "download", side_effect=subprocess.CalledProcessError(22, "curl")):
                with self.assertRaises(subprocess.CalledProcessError):
                    AVATAR.prepare(destination)
            self.assertFalse(destination.exists())

    def test_profile_must_point_to_gravatar_https_image(self):
        for url in ("http://gravatar.com/avatar/x", "https://example.com/avatar/x",
                    "https://gravatar.com.example.com/avatar/x", "https://gravatar.com/4o3f"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                AVATAR.avatar_url({"avatar_url": url})


if __name__ == "__main__":
    unittest.main()
