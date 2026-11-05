#!/bin/sh
# Run only on B. Private identity and complete snapshots never leave this process.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import base64, hashlib, json, os, pathlib, select, socket, ssl, struct, sys, time
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization


def emit(event, **values):
    print(
        json.dumps(
            {
                "event": event,
                "at_us": time.time_ns() // 1000,
                "monotonic_us": time.monotonic_ns() // 1000,
                **values,
            }
        ),
        flush=True,
    )


def varint(n):
    b = bytearray()
    while n > 127:
        b.append((n & 127) | 128)
        n >>= 7
    b.append(n)
    return bytes(b)


def field(k, b):
    return varint(k * 8 + 2) + varint(len(b)) + b


def parse(b):
    def number(i):
        n = 0
        s = 0
        while True:
            x = b[i]
            i += 1
            n |= (x & 127) << s
            if x < 128:
                return n, i
            s += 7

    o = {}
    i = 0
    while i < len(b):
        tag, i = number(i)
        k = tag >> 3
        wire = tag & 7
        if wire == 2:
            size, i = number(i)
            o[k] = b[i : i + size]
            i += size
        elif wire == 0:
            o[k], i = number(i)
        elif wire == 1:
            i += 8
        elif wire == 5:
            i += 4
        else:
            raise ValueError("wire")
    return o


state_path = pathlib.Path(sys.argv[1])
mode = sys.argv[2]
seconds = int(sys.argv[3])
interval = float(sys.argv[4]) if len(sys.argv) > 4 else 0.05
assert mode in ["flood", "slow"] and 1 <= seconds <= 600 and 0.001 <= interval <= 1
state = json.loads(state_path.read_text())
assert state["device_id"] and state["local"]
context = ssl.create_default_context(cafile="@BENCH_AUTH_DIR@/control-ca.crt")
raw = socket.create_connection(
    ("@BENCH_CONTROL_HOST@", @BENCH_CONTROL_PORT@), timeout=10
)
raw.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024 if mode == "slow" else 65536)
s = context.wrap_socket(raw, server_hostname="@BENCH_CONTROL_HOST@")
s.settimeout(3)
key = base64.b64encode(os.urandom(16)).decode()
s.sendall(
    (
        "GET /api/v2/device/control HTTP/1.1\r\nHost: @BENCH_CONTROL_HOST@:@BENCH_CONTROL_PORT@\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: "
        + key
        + "\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: natsume.control.v3\r\n\r\n"
    ).encode()
)
buffer = b""
while b"\r\n\r\n" not in buffer:
    buffer += s.recv(65536)
header, buffer = buffer.split(b"\r\n\r\n", 1)
assert header.split(b"\r\n")[0].split()[1] == b"101"
assert (
    base64.b64encode(
        hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()
    )
    in header
)


def exact(n):
    global buffer
    while len(buffer) < n:
        b = s.recv(max(4096, n - len(buffer)))
        if not b:
            raise EOFError()
        buffer += b
    b, buffer = buffer[:n], buffer[n:]
    return b


def receive():
    a, b = exact(2)
    size = b & 127
    if size == 126:
        size = struct.unpack("!H", exact(2))[0]
    elif size == 127:
        size = struct.unpack("!Q", exact(8))[0]
    mask = exact(4) if b & 128 else None
    payload = exact(size)
    if mask:
        payload = bytes(x ^ mask[i % 4] for i, x in enumerate(payload))
    return a & 15, payload


def send(op, payload):
    mask = os.urandom(4)
    n = len(payload)
    head = bytes([128 | op, 128 | (n if n < 126 else 126 if n < 65536 else 127)])
    if n >= 126:
        head += struct.pack("!H" if n < 65536 else "!Q", n)
    s.sendall(head + mask + bytes(x ^ mask[i % 4] for i, x in enumerate(payload)))


try:
    op, b = receive()
    assert op == 2
    nonce = parse(parse(b)[1])[1]
    assert len(nonce) == 32
    private = Ed25519PrivateKey.from_private_bytes(bytes(state["control_key"]))
    public = private.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )
    transcript = (
        b"NATSUME-DEVICE-CONTROL-CLIENT-PROOF\0"
        + b"\x01"
        + b"/api/v2/device/control\0"
        + b"natsume.control.v3\0"
        + nonce
        + public
        + b"\x02"
        + state["hardware_id"].encode()
    )
    proof = (
        field(1, b"2.12.0")
        + field(2, b"2.12.0")
        + field(3, state["hardware_id"].encode())
        + field(4, private.sign(hashlib.sha256(transcript).digest()))
        + field(6, b"")
    )
    send(2, field(1, proof))
    op, b = receive()
    assert op == 2
    session = parse(parse(b)[4])[1]
    assert len(session) == 16
    send(2, field(1, session) + field(2, bytes(state["local"])))
    op, b = receive()
    assert op == 2
    emit("probe_active", mode=mode, snapshot_bytes=len(state["local"]))
    s.settimeout(30 if mode == "slow" else 3)
    start = time.monotonic()
    deadline = start + seconds
    next_send = start
    next_ping = start + (30 if mode == "slow" else 0)
    sent = 1
    read = 1
    while time.monotonic() < deadline:
        now = time.monotonic()
        if now >= next_send:
            send(2, field(1, session) + field(2, bytes(state["local"])))
            sent += 1
            next_send = now + interval
        if now >= next_ping:
            send(9, session)
            next_ping = now + (30 if mode == "slow" else 1)
        if mode == "flood" and (
            buffer or s.pending() or select.select([s], [], [], 0)[0]
        ):
            op, b = receive()
            read += 1
            if op == 8:
                emit("probe_closed", sent=sent, received=read)
                break
            if op == 9:
                send(10, b)
        time.sleep(0.001)
    emit(
        "probe_finished",
        sent=sent,
        received=read,
        elapsed_us=int((time.monotonic() - start) * 1e6),
    )
except Exception as e:
    kind = type(e).__name__
    emit(
        "probe_error",
        sent=locals().get("sent", 0),
        received=locals().get("read", 0),
        kind=kind
        if kind
        in {
            "AssertionError",
            "TimeoutError",
            "EOFError",
            "BrokenPipeError",
            "ConnectionResetError",
            "SSLError",
            "KeyError",
            "ValueError",
        }
        else "other_error",
    )
    sys.exit(1)
finally:
    s.close()
PY
