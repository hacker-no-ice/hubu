#!/usr/bin/env python3
"""Reject direct Cargo dependencies across the Hubu/Gongbu runtime boundary."""

import json
from pathlib import Path
import subprocess
import sys


workspace_root = Path(__file__).resolve().parent.parent
result = subprocess.run(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
    cwd=workspace_root,
    check=True,
    capture_output=True,
    text=True,
)
metadata = json.loads(result.stdout)

violations = []
for package in metadata["packages"]:
    package_is_gongbu = package["name"].startswith("gongbu-")
    package_is_hubu = package["name"].startswith("hubu-")
    for dependency in package["dependencies"]:
        gongbu_to_hubu = package_is_gongbu and dependency["name"].startswith("hubu-")
        hubu_to_gongbu = package_is_hubu and dependency["name"].startswith("gongbu-")
        if gongbu_to_hubu or hubu_to_gongbu:
            violations.append(f"{package['name']} -> {dependency['name']}")

if violations:
    print("Cargo dependency boundary check failed:", file=sys.stderr)
    for violation in sorted(violations):
        print(f"  {violation}", file=sys.stderr)
    raise SystemExit(1)

print("validated no direct Cargo dependencies cross the Hubu/Gongbu boundary")
