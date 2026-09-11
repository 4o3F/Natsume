#!/usr/bin/env python3
"""Exercise postinstall file checks in temporary roots without host mutations."""
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parent


class DeploymentInputs(unittest.TestCase):
    def test_install_checks_inputs_without_writing_them(self):
        for package in ("client", "server"):
            with self.subTest(package=package), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                commands = root / "bin"
                commands.mkdir()
                for name in ("systemd-sysusers", "systemd-tmpfiles", "systemctl"):
                    stub = commands / name
                    stub.write_text("#!/bin/sh\nexit 0\n")
                    stub.chmod(0o755)
                script = root / "postinst"
                script.write_text(
                    (SOURCE / package / "scripts/postinstall.sh").read_text()
                    .replace("/etc/natsume", str(root / "etc/natsume"))
                )
                config_dir = "natsume" if package == "client" else "natsume-server"
                paths = [
                    root / "etc" / config_dir / "config.toml",
                    root / "etc/natsume/trust/control-ca.crt",
                    root / "etc/natsume/trust/local-origin-ca.crt",
                ]

                def install():
                    return subprocess.run(
                        ["/bin/sh", str(script), "configure"],
                        env={"PATH": str(commands), "LC_ALL": "C"},
                        capture_output=True, text=True, check=False,
                    )

                missing = install()
                self.assertEqual(missing.returncode, 0, missing.stderr)
                self.assertIn("service startup is deferred", missing.stderr)
                self.assertTrue(all(not path.exists() for path in paths))

                for path in paths:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(f"# deployment owns {path.name}\n")
                    path.chmod(0o644)
                paths[0].write_text((SOURCE / package / "config.example.toml").read_text())
                before = [(path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) for path in paths]
                for _ in range(2):
                    configured = install()
                    self.assertEqual(configured.returncode, 0, configured.stderr)
                    self.assertEqual(
                        [(path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) for path in paths],
                        before,
                    )

                for path, (content, _) in zip(paths, before):
                    path.write_bytes(b"")
                    self.assertNotEqual(install().returncode, 0)
                    self.assertEqual(path.read_bytes(), b"")
                    path.unlink()
                    path.mkdir()
                    self.assertNotEqual(install().returncode, 0)
                    self.assertTrue(path.is_dir())
                    path.rmdir()
                    path.write_bytes(content)
                for name in ("systemd-sysusers", "systemd-tmpfiles"):
                    (commands / name).write_text("#!/bin/sh\nexit 1\n")
                    self.assertNotEqual(install().returncode, 0)
                    (commands / name).write_text("#!/bin/sh\nexit 0\n")


if __name__ == "__main__":
    unittest.main()
