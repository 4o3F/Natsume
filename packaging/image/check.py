#!/usr/bin/env python3
"""Check this standalone image handoff directory and an optional matching Client Deb."""
import argparse
import csv
import re
import stat
import subprocess
import tarfile
from pathlib import Path
from urllib.parse import unquote, urlsplit

SOURCE = Path(__file__).resolve().parent
DESTINATION = "usr/share/natsume/image-integration"
SUPPORT_FILES = {"README.md", "inputs.md", "integration.md", "acceptance.md", "manifest.tsv", "check.py"}
CLIENT_ENTRY_LINES = {
    "etc/pam.d/gdm-contest": [
        "auth requisite pam_succeed_if.so user = teams quiet",
        "@include natsume-contest-admission", "@include gdm-autologin",
    ],
    "etc/pam.d/gdm-waiting": [
        "auth requisite pam_succeed_if.so user = waiting quiet", "@include gdm-autologin",
    ],
    "etc/pam.d/natsume-contest-admission": [
        f"{phase} requisite pam_exec.so quiet /usr/lib/natsume/natsume-privileged-helper pam-gate"
        for phase in ("auth", "account", "session")
    ],
    "usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf": [
        "ConditionUser=waiting", "ExecStart=/usr/bin/natsume-session-agent run",
    ],
}
CLIENT_PROGRAMS = {
    "usr/bin/natsume-device-daemon", "usr/bin/natsume-session-agent",
    "usr/lib/natsume/natsume-privileged-helper", "usr/lib/natsume/caddy",
}
SITE_FILES = {
    "etc/natsume/config.toml", "etc/natsume/site.toml", "etc/natsume/trust/control-ca.crt",
    "etc/natsume/trust/local-origin-ca.crt",
}


def check_source():
    with (SOURCE / "manifest.tsv").open() as source:
        reader = csv.DictReader(source, delimiter="\t")
        if reader.fieldnames != ["source", "destination", "mode", "action"]:
            raise ValueError("image manifest columns changed")
        rows = list(reader)
    paths = list(SOURCE.rglob("*"))
    if any(path.is_symlink() for path in paths):
        raise ValueError("handoff inputs must not depend on symlinks")
    files = {path.relative_to(SOURCE).as_posix(): path for path in paths if path.is_file()}
    declared = {row["source"] for row in rows}
    if len(rows) != len(declared) or set(files) != declared | SUPPORT_FILES:
        raise ValueError("handoff files differ from the deployment manifest and required documentation")
    for name, path in files.items():
        source_mode = 0o755 if name == "fragments/gdm/PostLogin.sh" else 0o644
        if stat.S_IMODE(path.stat().st_mode) != source_mode:
            raise ValueError(f"invalid input mode: {name}")
        if path.suffix == ".md":
            content = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
            for href in re.findall(r"\]\(([^)]+)\)", content):
                link = urlsplit(href)
                if link.scheme or link.netloc:
                    continue
                target = (path.parent / unquote(link.path)).resolve() if link.path else path
                if not target.is_relative_to(SOURCE) or not target.is_file():
                    raise ValueError(f"handoff document link leaves the bundle or is missing: {name}: {href}")
    for row in rows:
        if row["mode"] not in {"0600", "0644", "0755"}:
            raise ValueError(f"invalid deployment mode: {row['source']}")
        if row["action"] not in {"copy", "merge", "render", "initialize-home"}:
            raise ValueError(f"unknown image application method: {row['action']}")
        if row["action"] == "copy" and row["source"] != "rootfs" + row["destination"]:
            raise ValueError(f"rootfs destination mismatch: {row['source']}")
        configuration = "\n".join((row["source"], row["destination"], files[row["source"]].read_text()))
        if any(token in configuration for token in
               ("target-vm", "/tmp/natsume-", "/root/session-", "/home/contest", "natsume_contest")):
            raise ValueError(f"local VM or retired Home dependency in {row['source']}")
    subprocess.run(["sh", "-n", str(SOURCE / "fragments/gdm/PostLogin.sh")], check=True)
    return files, rows


def check_deb(deb, files, rows):
    package = subprocess.check_output(["dpkg-deb", "--field", str(deb), "Package"], text=True).strip()
    if package != "natsume-client":
        raise ValueError("expected a full natsume-client Deb")
    found = set()
    entries = set()
    all_paths = set()
    programs = set()
    with subprocess.Popen(["dpkg-deb", "--fsys-tarfile", str(deb)], stdout=subprocess.PIPE) as process:
        with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
            for entry in archive:
                name = entry.name.removeprefix("./")
                all_paths.add(name)
                if name in SITE_FILES:
                    raise ValueError("Client Deb contains image-owned site input: " + name)
                if name in CLIENT_ENTRY_LINES or name in CLIENT_PROGRAMS:
                    mode = 0o755 if name in CLIENT_PROGRAMS else 0o644
                    if not entry.isfile() or entry.uid != 0 or entry.gid != 0 or entry.mode != mode:
                        raise ValueError(f"invalid Client runtime entry: {name}")
                    if name in CLIENT_PROGRAMS:
                        programs.add(name)
                    else:
                        content = archive.extractfile(entry).read().decode().splitlines()
                        if not all(line in content for line in CLIENT_ENTRY_LINES[name]):
                            raise ValueError(f"Client role/PAM/Kiosk contract mismatch: {name}")
                        entries.add(name)
                prefix = DESTINATION + "/"
                if not name.startswith(prefix) or entry.isdir():
                    continue
                relative = name[len(prefix):]
                if relative not in files or not entry.isfile() or relative in found:
                    raise ValueError(f"unexpected image payload entry: {name}")
                source = files[relative]
                if entry.uid != 0 or entry.gid != 0 or entry.mode != stat.S_IMODE(source.stat().st_mode):
                    raise ValueError(f"incorrect packaged image input ownership/mode: {name}")
                stream = archive.extractfile(entry)
                if stream is None or stream.read() != source.read_bytes():
                    raise ValueError(f"packaged image input differs from source: {name}")
                found.add(relative)
        if process.wait() != 0:
            raise ValueError("dpkg-deb could not read the Client payload")
    if found != set(files):
        raise ValueError("Client Deb is missing handoff files: " + str(sorted(set(files) - found)))
    if programs != CLIENT_PROGRAMS or entries != set(CLIENT_ENTRY_LINES):
        raise ValueError("Client Deb is missing required programs or fixed session entry points")
    for row in rows:
        if row["destination"].lstrip("/") in all_paths:
            raise ValueError("Client Deb activates image-owned configuration: " + row["destination"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--deb", type=Path, help="also compare a complete matching Client Deb")
    args = parser.parse_args()
    files, rows = check_source()
    if args.deb:
        check_deb(args.deb, files, rows)
    print(f"image-inputs: {len(rows)} deployment inputs; {len(files)} standalone handoff files verified"
          + (" in Client Deb" if args.deb else ""))


if __name__ == "__main__":
    main()
