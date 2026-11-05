"""Configuration and non-networking checks for the workstation entry points."""

import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import bench_env

HERE = Path(__file__).resolve().parent
ENTRY_POINTS = [
    "server-run.sh",
    "server-api.sh",
    "server-fleet.sh",
    "resilience-run.sh",
    "control-probe.sh",
    "install-template.sh",
    "c1-run.sh",
    "c3-run.sh",
    "c5-run.sh",
    "c6-run.sh",
    "c6-cycle.sh",
    "fault-run.sh",
    "network-run.sh",
    "gdm-run.sh",
    "admission-run.sh",
    "target.sh",
    "fault-api.sh",
    "network-operations.sh",
    "fault-power.sh",
    "network-link.sh",
    "network-clock.sh",
    "run-vm.sh",
]


class ConfigurationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.work = Path(self.directory.name)
        self.env_file = self.work / "bench.env"
        self.env_file.write_text((HERE / "env.example").read_text())
        env = {
            "PATH": os.defpath,
            "PYTHONDONTWRITEBYTECODE": "1",
            "NATSUME_BENCH_ENV": str(self.env_file),
        }
        self.environment = patch.dict(os.environ, env, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def test_overrides_are_explicit_and_preview_is_public(self):
        os.environ["NATSUME_BENCH_VM1"] = "custom-one"
        cfg = bench_env.load()
        self.assertEqual(cfg["VM1"], "custom-one")
        self.assertEqual(cfg.port, 8443)
        self.assertNotIn("PASSWORD", cfg)

    def test_missing_device_fails_without_a_fallback(self):
        self.env_file.write_text(
            "\n".join(
                x
                for x in self.env_file.read_text().splitlines()
                if not x.startswith("NATSUME_BENCH_DEVICE1=")
            )
        )
        with self.assertRaisesRegex(
            SystemExit, "missing configuration: NATSUME_BENCH_DEVICE1"
        ):
            bench_env.load()

    def test_values_are_data_and_credentials_are_rejected(self):
        marker = self.work / "unexpected"
        self.env_file.write_text(
            self.env_file.read_text() + f"\nNATSUME_BENCH_PASSWORD=$(touch {marker})\n"
        )
        with self.assertRaises(SystemExit):
            bench_env.load()
        self.assertFalse(marker.exists())
        self.env_file.write_text((HERE / "env.example").read_text())
        os.environ["NATSUME_BENCH_SERVER_URL"] = "https://user:SENTINEL@example.org"
        with self.assertRaises(SystemExit) as error:
            bench_env.load()
        self.assertNotIn("SENTINEL", str(error.exception))

    def test_known_path_cannot_execute_shell_expansion(self):
        marker = self.work / "unexpected-path-command"
        lines = [
            line
            for line in self.env_file.read_text().splitlines()
            if not line.startswith("NATSUME_BENCH_VM_ROOT=")
        ]
        lines.append(f"NATSUME_BENCH_VM_ROOT='/srv/$(touch {marker})'")
        self.env_file.write_text("\n".join(lines))
        with self.assertRaises(SystemExit):
            bench_env.load()
        self.assertFalse(marker.exists())

    def test_malformed_url_error_does_not_echo_userinfo(self):
        os.environ["NATSUME_BENCH_SERVER_URL"] = "https://SENTINEL@／"
        with self.assertRaises(SystemExit) as error:
            bench_env.load()
        self.assertNotIn("SENTINEL", str(error.exception))

    def test_zero_port_and_duplicate_devices_are_invalid(self):
        os.environ["NATSUME_BENCH_SERVER_URL"] = "https://example.org:0"
        with self.assertRaises(SystemExit):
            bench_env.load()
        del os.environ["NATSUME_BENCH_SERVER_URL"]
        os.environ["NATSUME_BENCH_DEVICE2"] = bench_env.load()["DEVICE1"]
        with self.assertRaises(SystemExit):
            bench_env.load()

    def test_source_and_deployed_previews_do_not_connect_or_create_outputs(self):
        # A child process fails immediately if preview reaches a network/process API.
        blocker = self.work / "sitecustomize.py"
        blocker.write_text(
            "import socket,subprocess\n"
            'def blocked(*a,**k):raise RuntimeError("preview attempted I/O")\n'
            "socket.socket=blocked\nsubprocess.Popen=blocked\n"
        )
        env = dict(os.environ, PYTHONPATH=str(self.work))
        cfg = bench_env.load()
        output = self.work / "must-not-exist"
        for name in ENTRY_POINTS:
            for deployed in [False, True]:
                script = HERE / name
                if deployed:
                    script = self.work / name
                    script.write_text(cfg.script(HERE / name))
                with self.subTest(script=name, deployed=deployed):
                    r = subprocess.run(
                        ["/bin/sh", str(script), str(output), "--dry-run"],
                        capture_output=True,
                        text=True,
                        env=env,
                    )
                    self.assertEqual(r.returncode, 0, r.stderr)
                    self.assertEqual(
                        json.loads(r.stdout)["NATSUME_BENCH_DEVICE1"], cfg["DEVICE1"]
                    )
                    self.assertFalse(output.exists())

    def test_python_controller_previews_exit_before_actions(self):
        for name in [
            "fault_driver.py",
            "network_driver.py",
            "gdm_driver.py",
            "admission_driver.py",
            "server_driver.py",
            "resilience_driver.py",
        ]:
            with self.subTest(controller=name):
                r = subprocess.run(
                    [sys.executable, str(HERE / name), "--print-config"],
                    text=True,
                    capture_output=True,
                    env=dict(os.environ),
                )
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertEqual(json.loads(r.stdout)["NATSUME_BENCH_VM1"], "station01")

    def test_controller_commands_use_configured_names_and_paths(self):
        os.environ.update(
            NATSUME_BENCH_VM1="alpha",
            NATSUME_BENCH_VM2="beta",
            NATSUME_BENCH_VM_ROOT="/srv/reproduction",
            NATSUME_BENCH_RUN_ROOT="/home/bench/reproduction",
            NATSUME_BENCH_SERVER_URL="https://control.example.org:9443",
        )
        for name in ["fault_driver", "network_driver"]:
            sys.modules.pop(name, None)
        fault = importlib.import_module("fault_driver")
        network = importlib.import_module("network_driver")
        commands = []

        def capture(host, script, check=True):
            commands.append((host, script))
            return subprocess.CompletedProcess([], 0, stdout="{}\n")

        with patch.object(fault, "run", capture):
            fault.target("alpha", "reset_home")
        self.assertIn("/home/bench/reproduction/target.sh", commands[-1][1])
        self.assertIn(fault.IDs["alpha"], commands[-1][1])
        with patch.object(network, "run", capture):
            network.start()
        self.assertIn(
            "/srv/reproduction/run-vm.sh /srv/reproduction beta", commands[-1][1]
        )
        self.assertIn("natsume-vm-beta", commands[-1][1])
        self.assertEqual(network.CFG.host, "control.example.org")
        self.assertEqual(network.CFG.port, 9443)


if __name__ == "__main__":
    unittest.main()
