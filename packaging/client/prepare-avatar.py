#!/usr/bin/env python3
"""Fetch the public author avatar at package time, never during Cargo or startup."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import urlencode, urlsplit, urlunsplit

PROFILE_URL = "https://api.gravatar.com/v3/profiles/4o3f"
AVATAR_SIZE = 512


def download(url):
    return subprocess.run(
        ["curl", "--fail", "--silent", "--show-error", "--location",
         "--proto", "=https", "--proto-redir", "=https",
         "--connect-timeout", "10", "--max-time", "30",
         "--retry", "2", "--max-filesize", "2097152", url],
        check=True, capture_output=True,
    ).stdout


def avatar_url(profile):
    url = urlsplit(profile["avatar_url"])
    if (url.scheme != "https" or url.username or url.password
            or not (url.hostname == "gravatar.com"
                    or (url.hostname or "").endswith(".gravatar.com"))
            or not url.path.startswith("/avatar/")):
        raise ValueError("profile did not return a Gravatar HTTPS avatar URL")
    return urlunsplit(url._replace(query=urlencode({"s": AVATAR_SIZE, "d": "404"}), fragment=""))


def prepare(destination):
    payload = download(avatar_url(json.loads(download(PROFILE_URL))))
    # Reject error documents, wrong formats and unexpected sizes before packaging.
    if (payload[:16] != b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
            or payload[16:24] != AVATAR_SIZE.to_bytes(4, "big") * 2
            or not payload.endswith(b"\x00\x00\x00\x00IEND\xaeB\x60\x82")):
        raise ValueError("expected a complete 512 x 512 PNG avatar")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as temporary:
        temporary_path = Path(temporary.name)
        try:
            temporary.write(payload)
            temporary.close()
            temporary_path.chmod(0o644)
            temporary_path.replace(destination)
        finally:
            temporary_path.unlink(missing_ok=True)
    print(f"Author avatar: {destination} (512 x 512, SHA-256 {hashlib.sha256(payload).hexdigest()})")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: prepare-avatar.py OUTPUT.png")
    try:
        prepare(Path(sys.argv[1]))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"author avatar preparation failed: {error}") from error
