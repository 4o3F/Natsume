#!/bin/sh
# Run as the Server's unprivileged account; restore the original synthetic logo.
set -eu
exec python3 - "$@" <<'PY'
import hashlib
import json
import os
import pathlib
import random
import signal
import struct
import sys
import tempfile
import time
import zlib

if len(sys.argv) != 5 or os.geteuid() == 0:
    raise SystemExit(
        "Usage as service user: c5-logo.sh LOGO COUNT INTERVAL_SECONDS BACKUP"
    )
path = pathlib.Path(sys.argv[1])
count = int(sys.argv[2])
interval = float(sys.argv[3])
backup = pathlib.Path(sys.argv[4])
if (
    not path.is_absolute()
    or not backup.is_absolute()
    or not 10 <= count <= 30
    or not 1 <= interval <= 60
):
    raise SystemExit("invalid synthetic logo settings")
if path.is_symlink() or not path.is_file():
    raise SystemExit("regular source image required")
original = path.read_bytes()
with backup.open("xb") as f:
    f.write(original)
backup.chmod(0o600)


def publish(data):
    with tempfile.NamedTemporaryFile(
        dir=path.parent, prefix=".c5-", suffix=".tmp", delete=False
    ) as f:
        name = f.name
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.chmod(name, 0o644)
    os.replace(name, path)


def image(seed):
    rng = random.Random(seed)
    width = height = 256
    pixels = b"".join(b"\0" + rng.randbytes(width * 3) for _ in range(height))

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(pixels, 6))
        + chunk(b"IEND", b"")
    )


def stop(_signum, _frame):
    raise SystemExit("logo update interrupted")


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGHUP, stop)
print(
    json.dumps(
        {
            "original_sha256": hashlib.sha256(original).hexdigest(),
            "original_bytes": len(original),
            "updates": count,
            "interval_s": interval,
            "width": 256,
            "height": 256,
        }
    ),
    flush=True,
)
started = time.monotonic()
try:
    for index in range(count):
        delay = started + index * interval - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        data = image(20261104 + index)
        publish(data)
        print(
            json.dumps(
                {
                    "update": index + 1,
                    "at_unix_us": time.time_ns() // 1000,
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
            ),
            flush=True,
        )
    delay = started + count * interval - time.monotonic()
    if delay > 0:
        time.sleep(delay)
finally:
    publish(original)
    print(
        json.dumps(
            {
                "original_restored": hashlib.sha256(path.read_bytes()).digest()
                == hashlib.sha256(original).digest(),
                "restored_at_us": time.time_ns() // 1000,
            }
        ),
        flush=True,
    )
PY
