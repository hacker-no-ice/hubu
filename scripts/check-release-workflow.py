#!/usr/bin/env python3
"""Validate immutable unified-release coverage without publishing anything."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/release.yml"
PACKAGE_SCRIPT = ROOT / "scripts/package-release-archive.sh"
SMOKE_SCRIPT = ROOT / "scripts/verify-release-archive.sh"

PRODUCTION_BINARIES = (
    "hubu",
    "hubu-server",
    "hubu-mcp-server",
    "gongbu-server",
    "gongbu-mcp",
)
DEVELOPMENT_BINARIES = ("hubu-bench", "gongbu-sandbox")
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)


def fail(message: str) -> None:
    print(f"release workflow check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


workflow = WORKFLOW.read_text(encoding="utf-8")
package_script = PACKAGE_SCRIPT.read_text(encoding="utf-8")
smoke_script = SMOKE_SCRIPT.read_text(encoding="utf-8")
workflow_lines = [line.strip() for line in workflow.splitlines()]

for binary in PRODUCTION_BINARIES:
    build_flag = f"--bin {binary}"
    if workflow_lines.count(build_flag) != 2:
        fail(f"{build_flag!r} must appear in release checks and target builds")
    if not re.search(rf"(?m)^  {re.escape(binary)}$", package_script):
        fail(f"packaging does not enumerate {binary}")
    if not re.search(rf"(?m)^  {re.escape(binary)}$", smoke_script):
        fail(f"archive smoke does not enumerate {binary}")

for binary in DEVELOPMENT_BINARIES:
    if f"--bin {binary}" in workflow_lines:
        fail(f"development tool {binary} must not be built for release")
    if re.search(rf'cp .*[/"]{re.escape(binary)}', package_script):
        fail(f"development tool {binary} must not be copied into release archives")

for target in TARGETS:
    if workflow.count(f"target: {target}") != 2:
        fail(f"{target} must appear once in build and once in published smoke matrices")

for required in (
    "channel:",
    "default: canary",
    'INPUT_CHANNEL: ${{ inputs.channel }}',
    '"${INPUT_CHANNEL}" == "canary"',
    'git checkout --detach "${source_commit}"',
    "GONGBU_PRODUCT_VERSION:",
    "GONGBU_SOURCE_COMMIT:",
    "HUBU_PRODUCT_VERSION:",
    "HUBU_SOURCE_COMMIT:",
    "./scripts/test-release-packaging.sh",
    "./scripts/test-release-archive-runtime.sh",
    "./scripts/release-smoke-test.sh",
    "hubu-spend-executor-v4",
):
    if required not in workflow:
        fail(f"release workflow is missing {required!r}")

if not re.search(r"(?m)^permissions:\n  contents: read$", workflow):
    fail("workflow-wide permissions must remain contents: read")
if workflow.count("contents: write") != 1:
    fail("only the publish job may request contents: write")
if "cancel-in-progress: false" not in workflow:
    fail("immutable release publication must not be canceled mid-flight")

unpinned = []
for line_number, line in enumerate(workflow.splitlines(), start=1):
    match = re.search(r"\buses:\s*[^\s@]+@([^\s#]+)", line)
    if match and not re.fullmatch(r"[0-9a-f]{40}", match.group(1)):
        unpinned.append(f"line {line_number}: {line.strip()}")
if unpinned:
    fail("actions must be pinned to full commit SHAs: " + "; ".join(unpinned))

print(
    "validated immutable release workflow: five production binaries, four native "
    "targets, scheduled and explicit canaries, shared build identity, bounded "
    "permissions, and pinned actions"
)
