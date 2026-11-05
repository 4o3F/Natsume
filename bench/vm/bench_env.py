"""Load public benchmark settings and render deployment scripts without credentials."""

import json
import os
from pathlib import Path
import re
import shlex
import sys
from urllib.parse import urlsplit
import uuid

PREFIX = "NATSUME_BENCH_"
KEYS = (
    "A",
    "B",
    "VM_ROOT",
    "RUN_ROOT",
    "AUTH_DIR",
    "SERVER_URL",
    "ADMIN_LOGIN",
    "VM1",
    "VM2",
    "DEVICE1",
    "DEVICE2",
    "SEAT1",
    "SEAT2",
    "SERVER_DATA",
    "SERVER_LOGO",
    "CPUSET",
    "NUMA_NODE",
    "OVMF_CODE",
    "SSH_PORT_BASE",
    "VNC_DISPLAY_BASE",
    "MAC_PREFIX",
    "TEMPLATE_SSH_PORT",
    "TEMPLATE_VNC_DISPLAY",
)
HERE = Path(__file__).resolve().parent


def load():
    values = {}
    filename = os.environ.get(PREFIX + "ENV")
    if filename:
        try:
            lines = Path(filename).read_text().splitlines()
        except OSError:
            raise SystemExit("cannot read NATSUME_BENCH_ENV") from None
        for number, line in enumerate(lines, 1):
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            key, sep, raw = line.partition("=")
            key = key.strip()
            if not sep or key not in {PREFIX + k for k in KEYS}:
                raise SystemExit(f"invalid configuration key on line {number}")
            try:
                parts = shlex.split(raw, comments=True)
            except ValueError:
                raise SystemExit(
                    f"invalid configuration value on line {number}"
                ) from None
            if len(parts) != 1 or key in values:
                raise SystemExit(f"invalid or duplicate configuration key: {key}")
            values[key] = parts[0]
    for key in KEYS:
        name = PREFIX + key
        if name in os.environ:
            values[name] = os.environ[name]
        if not values.get(name):
            raise SystemExit(f"missing configuration: {name}")
    cfg = {key: values[PREFIX + key] for key in KEYS}
    patterns = {
        "A": r"[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9][A-Za-z0-9.-]*",
        "B": r"[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9][A-Za-z0-9.-]*",
        "VM1": r"[A-Za-z0-9][A-Za-z0-9-]*",
        "VM2": r"[A-Za-z0-9][A-Za-z0-9-]*",
        "SEAT1": r"S[0-9]{5}",
        "SEAT2": r"S[0-9]{5}",
        "ADMIN_LOGIN": r"[A-Za-z0-9_.-]+",
        "CPUSET": r"[0-9]+(?:[-,][0-9]+)*",
        "NUMA_NODE": r"[0-9]+",
        "TEMPLATE_SSH_PORT": r"[0-9]+",
        "TEMPLATE_VNC_DISPLAY": r"[0-9]+",
        "SSH_PORT_BASE": r"[0-9]+",
        "VNC_DISPLAY_BASE": r"[0-9]+",
        "MAC_PREFIX": r"(?:[0-9a-f]{2}:){4}[0-9a-f]{2}",
    }
    for key, pattern in patterns.items():
        if not re.fullmatch(pattern, cfg[key]):
            raise SystemExit(f"invalid configuration: {PREFIX + key}")
    for key in ["VM_ROOT", "RUN_ROOT", "AUTH_DIR", "SERVER_DATA", "OVMF_CODE"]:
        if (
            not re.fullmatch(r"/[A-Za-z0-9_./-]+", cfg[key])
            or ".." in Path(cfg[key]).parts
        ):
            raise SystemExit(f"invalid absolute path: {PREFIX + key}")
        cfg[key] = cfg[key].rstrip("/")
    if (
        not re.fullmatch(r"/[A-Za-z0-9_./ -]+", cfg["SERVER_LOGO"])
        or ".." in Path(cfg["SERVER_LOGO"]).parts
    ):
        raise SystemExit("invalid NATSUME_BENCH_SERVER_LOGO")
    for key in ["DEVICE1", "DEVICE2"]:
        try:
            parsed = uuid.UUID(cfg[key])
        except ValueError:
            raise SystemExit(f"invalid device UUID: {PREFIX + key}") from None
        if str(parsed) != cfg[key] or parsed.int == 0:
            raise SystemExit(f"invalid device UUID: {PREFIX + key}")
    if cfg["A"].split("@", 1)[1].lower() == cfg["B"].split("@", 1)[1].lower():
        raise SystemExit("A and B must be different SSH hosts")
    if (
        cfg["VM1"] == cfg["VM2"]
        or cfg["DEVICE1"] == cfg["DEVICE2"]
        or cfg["SEAT1"] == cfg["SEAT2"]
    ):
        raise SystemExit(
            "VM names, device IDs and seats must identify two different workstations"
        )
    try:
        url = urlsplit(cfg["SERVER_URL"])
        port = 443 if url.port is None else url.port
    except ValueError:
        raise SystemExit("invalid NATSUME_BENCH_SERVER_URL") from None
    if (
        not 1 <= port <= 65535
        or url.scheme != "https"
        or not url.hostname
        or url.username is not None
        or url.password is not None
        or url.path not in ["", "/"]
        or url.query
        or url.fragment
        or not re.fullmatch(r"[A-Za-z0-9.-]+", url.hostname)
    ):
        raise SystemExit(
            "NATSUME_BENCH_SERVER_URL must be an HTTPS origin without credentials"
        )
    if (
        int(cfg["SSH_PORT_BASE"]) + 99 > 65535
        or int(cfg["VNC_DISPLAY_BASE"]) + 99 + 5900 > 65535
    ):
        raise SystemExit("VM port range exceeds 65535")
    if (
        not 1 <= int(cfg["TEMPLATE_SSH_PORT"]) <= 65535
        or int(cfg["TEMPLATE_VNC_DISPLAY"]) + 5900 > 65535
    ):
        raise SystemExit("invalid template listener ports")
    cfg["SERVER_URL"] = cfg["SERVER_URL"].rstrip("/")
    return Config(cfg, url.hostname, port)


class Config(dict):
    def __init__(self, values, host, port):
        super().__init__(values)
        self.host = host
        self.port = port

    def expand(self, text):
        values = {**self, "CONTROL_HOST": self.host, "CONTROL_PORT": str(self.port)}
        for key, value in values.items():
            text = text.replace("@BENCH_" + key + "@", value)
        if re.search(r"@BENCH_[A-Z0-9_]+@", text):
            raise SystemExit("unresolved benchmark configuration token")
        return text

    def script(self, path):
        text = self.expand(Path(path).read_text())
        # Deployment copies are public, fully rendered and need no config module on A/B.
        if text.startswith("#!/"):
            first, text = text.split("\n", 1)
            preview = json.dumps({PREFIX + k: self[k] for k in KEYS}, indent=2)
            guard = (
                'for bench_arg in "$@"; do\n'
                '    case "$bench_arg" in --dry-run|--print-config)\n'
                "        printf '%s\\n' " + shlex.quote(preview) + "; exit 0 ;;\n"
                "    esac\ndone\n"
            )
            return first + "\n" + guard + "export NATSUME_BENCH_RENDERED=1\n" + text
        return text

    def preview(self):
        print(json.dumps({PREFIX + k: self[k] for k in KEYS}, indent=2))


def configured():
    cfg = load()
    if "--dry-run" in sys.argv or "--print-config" in sys.argv:
        cfg.preview()
        raise SystemExit(0)
    return cfg


def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ["exec", "render"]:
        raise SystemExit("Usage: bench_env.py exec|render SCRIPT [ARGS]")
    cfg = load()
    path = Path(sys.argv[2]).resolve()
    if path.parent != HERE or path.suffix != ".sh" or not path.is_file():
        raise SystemExit("only scripts next to bench_env.py can be rendered")
    args = sys.argv[3:]
    if "--dry-run" in args or "--print-config" in args:
        cfg.preview()
        return
    text = cfg.script(path)
    if sys.argv[1] == "render":
        sys.stdout.write(text)
        return
    env = dict(os.environ)
    env.update({PREFIX + k: cfg[k] for k in KEYS})
    env["PYTHONPATH"] = str(HERE) + (
        os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else ""
    )
    os.execvpe("/bin/sh", ["/bin/sh", "-c", text, str(path), *args], env)


if __name__ == "__main__":
    main()
