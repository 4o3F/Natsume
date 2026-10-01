#!/usr/bin/env python3
"""Run Operator browser acceptance against an isolated production HTTPS Server.

Linux prerequisites: bubblewrap, OpenSSL, Cargo, pnpm and Playwright Chromium.
Only the fresh fixture is writable inside the Server mount namespace. No host
configuration, Web installation, trust store or existing database is changed.
"""

import base64
from contextlib import closing
from datetime import datetime, timedelta, timezone
import fcntl
import hashlib
import http.client
import json
import os
from pathlib import Path
import pty
import re
import secrets
import select
import shutil
import signal
import socket
import sqlite3
import ssl
import subprocess
import tempfile
import termios
import time


REPO = Path(__file__).resolve().parent.parent


def run(args, **kwargs):
    return subprocess.run(args, check=True, cwd=REPO, **kwargs)


def openssl(*args, input=None):
    return run(["openssl", *map(str, args)], input=input, capture_output=True).stdout


def prepare(root):
    keys = root / "keys"
    keys.mkdir()
    (root / "config").mkdir()
    for name in ("control", "origin"):
        openssl(
            "req", "-x509", "-newkey", "ec", "-pkeyopt",
            "ec_paramgen_curve:prime256v1", "-nodes", "-days", "30",
            "-keyout", keys / f"{name}.pem", "-out", root / f"{name}-ca.crt",
            "-subj", f"/CN=Natsume Operator Acceptance {name} CA",
            "-addext", "basicConstraints=critical,CA:TRUE",
            "-addext", "keyUsage=critical,keyCertSign,cRLSign",
        )
    openssl("req", "-new", "-newkey", "ec", "-pkeyopt",
            "ec_paramgen_curve:prime256v1", "-nodes", "-keyout", keys / "tls.pem",
            "-out", root / "tls.csr", "-subj", "/CN=Natsume Operator Acceptance Server")
    extension = root / "tls.ext"
    extension.write_text(
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
        "extendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n"
    )
    openssl("x509", "-req", "-in", root / "tls.csr", "-CA", root / "control-ca.crt",
            "-CAkey", keys / "control.pem", "-set_serial", "1", "-days", "7",
            "-extfile", extension, "-out", root / "tls.crt")
    for source, target in ((root / "tls.crt", keys / "server-tls-leaf.der"),
                           (root / "origin-ca.crt", keys / "origin-ca.der")):
        openssl("x509", "-in", source, "-outform", "DER", "-out", target)
    for name, target in (("tls", "server-tls-key.pk8"), ("origin", "origin-ca-key.pk8")):
        openssl("pkcs8", "-topk8", "-nocrypt", "-in", keys / f"{name}.pem",
                "-outform", "DER", "-out", keys / target)
    public = openssl("x509", "-in", root / "tls.crt", "-pubkey", "-noout")
    spki = openssl("pkey", "-pubin", "-outform", "DER", input=public)
    pin = base64.b64encode(hashlib.sha256(spki).digest()).decode()
    with socket.socket() as available:
        available.bind(("127.0.0.1", 0))
        port = available.getsockname()[1]
    now = datetime.now(timezone.utc)
    expiry = (now + timedelta(days=3)).strftime("%Y-%m-%dT%H:%M:%SZ")
    contest = (now + timedelta(days=1)).strftime("%Y-%m-%dT%H:%M:%SZ")
    (root / "config/config.toml").write_text(f'''[listen]
https = "127.0.0.1:{port}"
[log]
level = "trace"
[storage]
database = "{root}/natsume.db"
root_key = "{keys}/server-root.key"
organization_logos = "{root}/logos"
[tls]
certificate = "{keys}/server-tls-leaf.der"
private_key = "{keys}/server-tls-key.pk8"
[site]
gateway_hostname = "gateway.example.test"
gateway_not_after = "{expiry}"
contest_end = "{contest}"
[trust]
control_root = "{root}/control-ca.crt"
local_origin_root = "{root}/origin-ca.crt"
[runtime]
domjudge_origin = "https://judge.example.test"
''')
    return port, pin


def command(root, mode, *options):
    return [
        "bwrap", "--die-with-parent", "--ro-bind", "/", "/",
        "--bind", str(root), str(root), "--dev-bind", "/dev", "/dev",
        "--tmpfs", "/etc", "--ro-bind", str(root / "config"), "/etc/natsume-server",
        "--tmpfs", "/usr/share", "--ro-bind", str(REPO / "web/dist"),
        "/usr/share/natsume-server/web", *options, "--", str(REPO / "target/debug/natsume-server"), mode,
    ]


def bootstrap(root, password, environment):
    master, slave = pty.openpty()

    def terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    process = subprocess.Popen(command(root, "bootstrap"), stdin=slave, stdout=slave,
                               stderr=slave, env=environment, preexec_fn=terminal)
    os.close(slave)
    output = b""
    try:
        for prompt, value in ((b"Login name: ", "acceptance-admin"),
                              (b"Password: ", password), (b"Confirm password: ", password)):
            deadline = time.monotonic() + 30
            while prompt not in output:
                if time.monotonic() >= deadline or process.poll() is not None:
                    raise RuntimeError("TTY bootstrap did not reach the expected prompt")
                if select.select([master], [], [], 1)[0]:
                    try:
                        output += os.read(master, 65536)
                    except OSError as error:
                        raise RuntimeError("TTY bootstrap terminated early") from error
            os.write(master, (value + "\n").encode())
        # Drain the terminal while waiting, without printing credential input.
        deadline = time.monotonic() + 30
        while process.poll() is None:
            if time.monotonic() >= deadline:
                raise RuntimeError("TTY bootstrap did not finish")
            if select.select([master], [], [], 1)[0]:
                try:
                    output += os.read(master, 65536)
                except OSError:
                    break
        if process.wait(timeout=10) != 0 or password.encode() in output:
            raise RuntimeError("TTY bootstrap failed or echoed the password")
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        os.close(master)


def wait_ready(port, trust, server):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if server.poll() is not None:
            raise RuntimeError("production HTTPS Server exited during startup")
        try:
            with closing(http.client.HTTPSConnection("127.0.0.1", port, context=trust, timeout=2)) as client:
                client.connect()
                protocol = client.sock.version()
                client.request("GET", "/api/v2/health")
                response = client.getresponse()
                if response.status == 200 and json.loads(response.read()) == {"status": "ok"}:
                    if protocol != "TLSv1.3":
                        raise RuntimeError("the production listener did not negotiate TLS 1.3")
                    return
        except (OSError, http.client.HTTPException):
            time.sleep(0.1)  # Startup readiness only; never synchronizes a business race.
    raise RuntimeError("the trusted HTTPS health probe timed out")


def request_limits(port, trust):
    result = {}
    with closing(http.client.HTTPSConnection("127.0.0.1", port, context=trust, timeout=15)) as client:
        client.request("POST", "/api/v2/operator/register/inspect", "x" * (24 * 1024 + 1),
                       {"Content-Type": "application/json"})
        response = client.getresponse()
        payload = response.read()
        if response.status != 413 or len(payload) > 128 or response.getheader("cache-control") != "no-store":
            raise RuntimeError(f"the HTTPS body limit contract failed (status={response.status}, body_bytes={len(payload)}, cache={response.getheader('cache-control')})")
        result["oversized_status"] = response.status
        result["oversized_body_bytes"] = len(payload)
    with closing(http.client.HTTPSConnection("127.0.0.1", port, context=trust, timeout=15)) as client:
        client.putrequest("POST", "/api/v2/operator/register/inspect")
        client.putheader("Content-Type", "application/json")
        client.putheader("Content-Length", str(8 * 1024 * 1024 + 4097))
        client.endheaders()
        response = client.getresponse()
        response.read()
        if response.status != 413 or response.getheader("cache-control") != "no-store":
            raise RuntimeError(f"the HTTPS outer body limit contract failed (status={response.status}, cache={response.getheader('cache-control')})")
        result["outer_ingress_oversized_status"] = response.status
    with closing(http.client.HTTPSConnection("127.0.0.1", port, context=trust, timeout=15)) as client:
        client.connect()
        client.putrequest("POST", "/api/v2/operator/register/inspect")
        client.putheader("Content-Type", "application/json")
        client.putheader("Content-Length", "20")
        client.endheaders()
        started = time.monotonic()
        client.send(b"{")
        response = client.getresponse()
        elapsed = time.monotonic() - started
        payload = response.read()
        if response.status != 408 or payload or response.getheader("cache-control") != "no-store":
            raise RuntimeError(f"the HTTPS body deadline contract failed (status={response.status}, body_bytes={len(payload)}, cache={response.getheader('cache-control')})")
        result.update(slow_body_status=response.status, slow_body_elapsed_seconds=round(elapsed, 3))
    return result


def main():
    os.umask(0o077)
    for executable in ("bwrap", "openssl", "cargo", "pnpm"):
        if not shutil.which(executable):
            raise RuntimeError(f"missing acceptance prerequisite: {executable}")
    run(["cargo", "build", "-p", "natsume-server", "--bin", "natsume-server", "--locked"])
    run(["pnpm", "--filter", "@natsume/web", "build"])
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    evidence = REPO / "context/operator-management-p5" / f"{stamp}-{secrets.token_hex(3)}"
    evidence.mkdir(parents=True)
    environment = dict(os.environ, OTEL_SDK_DISABLED="true")
    with tempfile.TemporaryDirectory(prefix="natsume-operator-https-") as directory:
        root = Path(directory)
        port, pin = prepare(root)
        password = secrets.token_hex(12) + "1@"
        print("Bootstrapping the isolated production Server through a real TTY", flush=True)
        bootstrap(root, password, environment)
        trust = ssl.create_default_context(cafile=str(root / "control-ca.crt"))
        with (root / "server.log").open("wb") as log:
            with (root / "server-info.json").open("wb") as info:
                server = subprocess.Popen(command(root, "serve", "--info-fd", str(info.fileno())),
                                          stdout=log, stderr=log, pass_fds=(info.fileno(),),
                                          env=environment, start_new_session=True)
            server_pid = None
            try:
                wait_ready(port, trust, server)
                server_pid = json.loads((root / "server-info.json").read_text())["child-pid"]
                limits = request_limits(port, trust)
                print("Trusted TLS 1.3 health, body limit and body deadline probes passed", flush=True)
                environment.update(
                    NATSUME_OPERATOR_HTTPS_ORIGIN=f"https://127.0.0.1:{port}",
                    NATSUME_OPERATOR_HTTPS_SPKI=pin,
                    NATSUME_OPERATOR_HTTPS_ROOT=str(root),
                    NATSUME_OPERATOR_HTTPS_EVIDENCE=str(evidence),
                    NATSUME_OPERATOR_TEST_PASSWORD=password,
                )
                with (evidence / "playwright.log").open("wb") as browser_log:
                    result = subprocess.run(["pnpm", "exec", "playwright", "test", "--config",
                                             "playwright.https.config.ts"], cwd=REPO / "web",
                                            env=environment, stdout=browser_log, stderr=browser_log)
                # Playwright failure call logs can include fill values and link fragments.
                for artifact in evidence.rglob("*"):
                    if artifact.suffix not in (".log", ".md"):
                        continue
                    browser_output = artifact.read_text()
                    for secret in (password + "@2", password + "@3", password + "@4", password,
                                   "1" * 15 + "@", "incorrect-current-password"):
                        browser_output = browser_output.replace(secret, "[redacted]")
                    browser_output = re.sub(r"(?:invite|reset)_[0-9a-f]{64}", "[redacted-link]", browser_output)
                    browser_output = re.sub(r"\b[0-9a-f]{64}\b", "[redacted-credential]", browser_output)
                    artifact.write_text(browser_output)
                if result.returncode:
                    raise RuntimeError(f"real HTTPS browser acceptance failed; evidence: {evidence}")
            finally:
                if server.poll() is None:
                    # Let Server complete shutdown before its bubblewrap parent exits.
                    if server_pid is not None:
                        try:
                            os.kill(server_pid, signal.SIGTERM)
                        except ProcessLookupError:
                            pass
                    else:
                        os.killpg(server.pid, signal.SIGTERM)
                    try:
                        server.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        os.killpg(server.pid, signal.SIGKILL)
                        server.wait()
        with sqlite3.connect(root / "natsume.db") as database:
            counts = {table: database.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                      for table in ("operator_accounts", "operator_sessions", "operator_invitations", "operator_password_resets")}
            hashes_only = database.execute("SELECT count(*) FROM operator_invitations WHERE length(token_hash) != 32").fetchone()[0] == 0
            hashes_only &= database.execute("SELECT count(*) FROM operator_password_resets WHERE length(token_hash) != 32").fetchone()[0] == 0
            hashes_only &= database.execute("SELECT count(*) FROM operator_sessions WHERE length(session_credential_hash) != 32").fetchone()[0] == 0
        if password in (root / "server.log").read_text() or not hashes_only:
            raise RuntimeError("fixture secrets were found in ordinary output or storage")
        shutil.copyfile(root / "server.log", evidence / "server.log")
        if "graceful shutdown completed" not in (root / "server.log").read_text():
            raise RuntimeError("the isolated Server did not finish graceful shutdown")
        summary = {
            "commit": run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip(),
            "server_binary_sha256": hashlib.sha256((REPO / "target/debug/natsume-server").read_bytes()).hexdigest(),
            "web_index_sha256": hashlib.sha256((REPO / "web/dist/index.html").read_bytes()).hexdigest(),
            "origin": f"https://127.0.0.1:{port}", "tls": "TLSv1.3", "ca_and_ip_verified": True,
            "browser_fixture_leaf_spki_pinned": True, "request_limits": limits,
            "final_row_counts": counts, "session_and_invitation_hashes_are_32_bytes": hashes_only,
            "graceful_shutdown": "graceful shutdown completed" in (root / "server.log").read_text(),
            "temporary_keys_database_and_password_removed_on_exit": True,
            "acceptance_level": "isolated production Server HTTPS plus Chromium; not deployed-origin acceptance",
        }
        (evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"Real HTTPS acceptance passed. Evidence: {evidence}", flush=True)


if __name__ == "__main__":
    main()
