"""Check the Server API projection without opening credentials or connections."""

import ast
import copy
from pathlib import Path
import unittest
import uuid


class ProjectionTests(unittest.TestCase):
    def setUp(self):
        source = (Path(__file__).parent / "server-api.sh").read_text()
        code = source.split("<<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
        tree = ast.parse(code)
        nodes = [
            n
            for n in tree.body
            if isinstance(n, ast.FunctionDef)
            and n.name == "selected"
            or isinstance(n, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == "ENUM" for t in n.targets)
        ]
        namespace = {"uuid": uuid}
        exec(
            compile(ast.Module(body=nodes, type_ignores=[]), "projection", "exec"),
            namespace,
        )
        self.selected = namespace["selected"]
        self.row = {
            "device_id": "11111111-1111-4111-8111-111111111111",
            "machine_hardware_id": "22222222-2222-5222-8222-222222222222",
            "evidence_quality": "strong",
            "state": "enabled",
            "convergence": {
                "connection_state": "active",
                "received_at_unix_ms": 123,
                "binding": {
                    "status": "converged",
                    "actual": {"assignment_state": "applied"},
                },
                "gateway": {"status": "converged", "actual": {"state": "ready"}},
                "session_control": {
                    "status": "converged",
                    "target": {"terminate_epoch": 4, "foreground_target": "contest"},
                    "actual": {
                        "foreground": "contest",
                        "waiting_ready": False,
                        "contest_ready": True,
                        "completed_terminate_epoch": 4,
                    },
                },
                "home": {
                    "status": "converged",
                    "target_reset_epoch": 7,
                    "actual": {"state": "steady", "completed_reset_epoch": 7},
                },
            },
        }

    def test_epochs_and_foreground_are_preserved(self):
        out = self.selected(self.row)
        self.assertEqual(out["home"]["target_reset_epoch"], 7)
        self.assertEqual(out["session_control"]["target_terminate_epoch"], 4)
        self.assertFalse(out["session_control"]["waiting_ready"])
        self.assertTrue(out["session_control"]["contest_ready"])

    def test_unknown_fields_and_text_cannot_escape(self):
        row = copy.deepcopy(self.row)
        row["candidate_public_key"] = "PRIVATE_SENTINEL"
        row["network"] = {"address": "PRIVATE_SENTINEL"}
        for name in ["binding", "gateway", "session_control", "home"]:
            row["convergence"][name]["password"] = "PRIVATE_SENTINEL"
            row["convergence"][name]["actual"]["diagnostic"] = "PRIVATE_SENTINEL"
            row["convergence"][name]["actual"]["state"] = "PRIVATE_SENTINEL"
        self.assertNotIn("PRIVATE_SENTINEL", str(self.selected(row)))
        self.assertNotIn("network", self.selected(row))


if __name__ == "__main__":
    unittest.main()
