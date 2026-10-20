#!/usr/bin/env python3
"""Prepare verified, fixed runtime assets at build time, never on installation."""

import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import tarfile
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
REPOSITORY = HERE.parent.parent


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fetch(cache, filename, url, sha256):
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / filename
    if not path.exists():
        if not url.startswith("https://"):
            raise ValueError("asset download must use HTTPS")
        temporary = path.with_suffix(path.suffix + ".part")
        try:
            with urllib.request.urlopen(url, timeout=90) as response, temporary.open("wb") as output:
                shutil.copyfileobj(response, output)
            if digest(temporary) != sha256:
                raise ValueError(f"checksum mismatch: {filename}")
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if digest(path) != sha256:
        raise ValueError(f"checksum mismatch: {filename}; remove corrupt cache explicitly")
    return path


def prepare(directory):
    manifest = json.loads((HERE / "assets.json").read_text())
    downloads = {name: fetch(directory / "downloads", item["filename"], item["url"], item["sha256"])
                 for name, item in manifest.items()}
    runtime = directory / "runtime"
    (runtime / "fonts").mkdir(parents=True, exist_ok=True)
    (runtime / "licenses").mkdir(exist_ok=True)
    # Read only the three named regular members, not an upstream directory tree.
    with tarfile.open(downloads["typst"], "r:xz") as archive:
        for name in ["typst", "LICENSE", "NOTICE"]:
            member = archive.getmember(f"typst-x86_64-unknown-linux-musl/{name}")
            if not member.isfile():
                raise ValueError("unexpected Typst archive member")
            target = runtime / "typst" if name == "typst" else runtime / "licenses" / f"Typst-{name}"
            with archive.extractfile(member) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            target.chmod(0o755 if name == "typst" else 0o644)
    if digest(runtime / "typst") != manifest["typst"]["binary_sha256"]:
        raise ValueError("Typst executable checksum mismatch")
    version = subprocess.check_output([str(runtime / "typst"), "--version"], text=True).strip()
    if version != "typst 0.14.2 (b33de9de)":
        raise ValueError("unexpected Typst version")
    shutil.copyfile(downloads["font"], runtime / "fonts" / manifest["font"]["filename"])
    shutil.copyfile(downloads["font_license"], runtime / "licenses" / "Noto-OFL.txt")
    # Tracked license copies must be identical to the verified upstream assets.
    for local, upstream in [("LICENSE", "Typst-LICENSE"), ("NOTICE", "Typst-NOTICE"), ("Noto-OFL.txt", "Noto-OFL.txt")]:
        if digest(HERE / "licenses" / local) != digest(runtime / "licenses" / upstream):
            raise ValueError(f"tracked license differs: {local}")
    shutil.copyfile(HERE / "template.typ", runtime / "template.typ")
    print(f"Verified {version}, Noto Sans Mono CJK SC Sans2.004 and licenses: {runtime}")
    return runtime


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=pathlib.Path, default=REPOSITORY / "target" / "print-domjudge")
    arguments = parser.parse_args()
    prepare(arguments.directory.resolve())
