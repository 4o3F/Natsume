#!/usr/bin/env python3
"""Check repository ignore rules, then run the standalone image handoff checker."""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "packaging/image"

files = sorted(path for path in SOURCE.rglob("*") if path.is_file())
result = subprocess.run(
    ["git", "check-ignore", "--no-index", "--stdin"], cwd=ROOT,
    input="\n".join(str(path.relative_to(ROOT)) for path in files) + "\n",
    text=True, capture_output=True,
)
if result.returncode != 1:
    raise SystemExit("image build inputs are ignored or could not be checked: " + result.stdout)
subprocess.run([sys.executable, str(SOURCE / "check.py"), *sys.argv[1:]], check=True)
