#!/usr/bin/env python3
"""Validate public-facing metadata for every unified workspace package."""

import json
from pathlib import Path
import subprocess
import sys


REPOSITORY = "https://github.com/hacker-no-ice/hubu"
HOMEPAGE = f"{REPOSITORY}#readme"
DOCUMENTATION = f"{REPOSITORY}/tree/main/docs"
PLACEHOLDER_MARKERS = ("your-org", "example.com/hubu", "private/hubu")
EXPECTED_PACKAGES = {
    "gongbu-api",
    "gongbu-build-info",
    "gongbu-mcp",
    "hubu-api",
    "hubu-bench",
    "hubu-cli",
    "hubu-common",
    "hubu-core",
    "hubu-executor-contract",
    "hubu-mcp",
    "hubu-wallet",
}
EXPECTED_RUST_VERSION = "1.88"


def fail(message: str) -> None:
    print(f"Cargo metadata check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


workspace_root = Path(__file__).resolve().parent.parent
result = subprocess.run(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
    cwd=workspace_root,
    check=True,
    capture_output=True,
    text=True,
)
metadata = json.loads(result.stdout)

if not metadata["packages"]:
    fail("workspace contains no packages")

package_names = {package["name"] for package in metadata["packages"]}
if package_names != EXPECTED_PACKAGES:
    fail(
        "workspace package set differs from the unified graph: "
        f"expected {sorted(EXPECTED_PACKAGES)!r}, got {sorted(package_names)!r}"
    )

for package in metadata["packages"]:
    name = package["name"]
    expected_links = {
        "repository": REPOSITORY,
        "homepage": HOMEPAGE,
        "documentation": DOCUMENTATION,
    }
    for field, expected in expected_links.items():
        if package[field] != expected:
            fail(f"{name} {field} is {package[field]!r}; expected {expected!r}")

    if not package["description"]:
        fail(f"{name} has no description")
    if not package["keywords"]:
        fail(f"{name} has no keywords")
    if not package["categories"]:
        fail(f"{name} has no categories")
    if package["authors"]:
        fail(f"{name} invents or overrides workspace authorship: {package['authors']!r}")
    if package["publish"] != []:
        fail(f"{name} must remain non-publishable, got {package['publish']!r}")
    if package["rust_version"] != EXPECTED_RUST_VERSION:
        fail(
            f"{name} rust-version is {package['rust_version']!r}; "
            f"expected {EXPECTED_RUST_VERSION!r}"
        )

for manifest in workspace_root.glob("**/Cargo.toml"):
    if "target" in manifest.parts:
        continue
    contents = manifest.read_text(encoding="utf-8").lower()
    for marker in PLACEHOLDER_MARKERS:
        if marker in contents:
            fail(f"{manifest.relative_to(workspace_root)} contains forbidden marker {marker!r}")

print(f"validated public metadata for {len(metadata['packages'])} workspace packages")
