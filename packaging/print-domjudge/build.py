#!/usr/bin/env python3
"""Build the independent amd64 wrapper Debian package with pinned nFPM."""

import argparse
import os
import pathlib
import platform
import re
import shutil
import string
import subprocess
import tarfile
import tempfile
import tomllib

from assets import HERE, REPOSITORY, fetch, prepare


def package_inputs(environment):
    if environment.get("ARCH", "amd64") != "amd64":
        raise ValueError("ARCH must be amd64")
    manifest = tomllib.loads((REPOSITORY / "Cargo.toml").read_text())
    version = environment.get("VERSION", manifest["workspace"]["package"]["version"])
    if not re.fullmatch(r"[0-9][A-Za-z0-9.+:~\-]*", version):
        raise ValueError("VERSION must be a nonempty Debian package version")
    prebuilt = environment.get("RUST_RELEASE_DIR")
    if prebuilt is not None and not prebuilt:
        raise ValueError("RUST_RELEASE_DIR must not be empty")
    release = pathlib.Path(prebuilt) if prebuilt is not None else pathlib.Path("target/release")
    binary = (REPOSITORY / release / "natsume-print-domjudge").resolve()
    return version, binary, prebuilt is None


def shared_library_dependencies(binaries, directory):
    # Analyze BOTH executables. Typst's pinned musl binary is currently static.
    # Debian symbol metadata declares actual builder ABI requirements, not OS support policy.
    with tempfile.TemporaryDirectory(dir=directory) as temporary:
        root = pathlib.Path(temporary)
        (root / "debian").mkdir()
        (root / "debian" / "control").write_text(
            "Source: natsume-print-domjudge\nSection: utils\nPriority: optional\n"
            "Maintainer: Natsume Project\n\nPackage: natsume-print-domjudge\n"
            "Architecture: amd64\nDescription: DOMjudge wrapper\n")
        package = root / "debian" / "natsume-print-domjudge"
        (package / "DEBIAN").mkdir(parents=True)
        command = ["dpkg-shlibdeps", "-O"]
        for binary in binaries:
            staged = package / "usr" / "bin" / binary.name
            staged.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(binary, staged)
            staged.chmod(0o755)
            command.extend(["-e", str(staged.relative_to(root))])
        output = subprocess.check_output(command, cwd=root, text=True)
    prefix = "shlibs:Depends="
    if not output.startswith(prefix):
        raise ValueError("unexpected shared-library dependency output")
    return "\n".join(f"  - {dependency}" for dependency in output.removeprefix(prefix).strip().split(", ") if dependency)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=REPOSITORY / "dist" / "packages")
    args = parser.parse_args()
    if platform.machine() != "x86_64" or platform.system() != "Linux":
        parser.error("build on Linux amd64; cross packaging is not supported")
    try:
        version, binary, build_wrapper = package_inputs(os.environ)
    except ValueError as error:
        parser.error(str(error))
    directory = REPOSITORY / "target" / "print-domjudge"
    runtime = prepare(directory)
    nfpm_version = (REPOSITORY / "packaging" / "nfpm.version").read_text().strip()
    checksum, filename = (REPOSITORY / "packaging" / "nfpm.sha256").read_text().split()
    if filename != f"nfpm_{nfpm_version}_Linux_x86_64.tar.gz":
        raise ValueError("nFPM pin mismatch")
    archive = fetch(directory / "downloads", filename,
                    f"https://github.com/goreleaser/nfpm/releases/download/v{nfpm_version}/{filename}", checksum)
    nfpm = directory / "nfpm"
    with tarfile.open(archive, "r:gz") as source:
        member = source.getmember("nfpm")
        if not member.isfile():
            raise ValueError("unexpected nFPM archive member")
        with source.extractfile(member) as stream, nfpm.open("wb") as output:
            output.write(stream.read())
    nfpm.chmod(0o755)
    if build_wrapper:
        subprocess.run(["cargo", "build", "--locked", "--release", "--package", "natsume-print-domjudge",
                        "--bin", "natsume-print-domjudge", "--target-dir", str(REPOSITORY / "target")],
                       cwd=REPOSITORY, check=True)
    args.output.mkdir(parents=True, exist_ok=True)
    environment = os.environ | {
        "VERSION": version,
        "WRAPPER_BIN": str(binary),
        "SHLIBS_DEPENDS": shared_library_dependencies([binary, runtime / "typst"], directory),
        "RUNTIME_ASSETS": str(runtime),
    }
    rendered = directory / "nfpm.yaml"
    rendered.write_text(string.Template((HERE / "nfpm.yaml").read_text()).substitute(environment))
    subprocess.run([str(nfpm), "package", "--packager", "deb", "--config", str(rendered),
                    "--target", str(args.output.resolve())], cwd=REPOSITORY, env=environment, check=True)
    print(f"Package version {version} built from {binary}. Inspect contents and run lifecycle acceptance separately.")


if __name__ == "__main__":
    main()
