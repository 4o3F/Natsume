#!/bin/sh
# Run on B after stopping loadgen; export a closed subset of numeric metrics.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - <<'PY'
import csv, gzip, json, pathlib

path = pathlib.Path("@BENCH_RUN_ROOT@/server-reliability/events/events.csv.gz")
events = {
    "tcp_tls",
    "wss_upgrade",
    "handshake",
    "first_snapshot",
    "first_target",
    "disconnect",
    "reconnect_delay",
    "ping_rtt",
}
codes = {
    "OK",
    "INITIAL",
    "RETRY",
    "RESUME",
    "ENROLLMENT",
    "SENT",
    "RECEIVED",
    "EOF",
    "CLOSE",
    "IO",
    "TLS",
    "WEBSOCKET",
    "PEER",
    "SERVER_SILENCE",
    "HANDSHAKE_SILENCE",
    "SESSION_FENCED",
    "CONNECT",
    "CONNECT_TIMEOUT",
    "WSS_UPGRADE",
    "HTTP_101",
    "101",
    "WS",
    "NET",
    "TIMEOUT",
}
rejected = 0
with gzip.open(path, "rt") as f:
    for row in csv.DictReader(f):
        if row["event_kind"] not in events or row["detail_code"] not in codes:
            rejected += 1
            continue
        if not all(
            row[k].lstrip("-").isdigit() for k in ["ts_us", "device", "latency_us"]
        ):
            rejected += 1
            continue
        print(
            json.dumps(
                {
                    "at_us": int(row["ts_us"]),
                    "device_index": int(row["device"]),
                    "event": row["event_kind"],
                    "elapsed_us": int(row["latency_us"]),
                    "code": row["detail_code"],
                }
            )
        )
print(json.dumps({"kind": "filter_summary", "rejected_rows": rejected}))
PY
