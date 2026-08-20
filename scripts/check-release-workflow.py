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
    "hubu-unified-mcp",
    "gongbu-server",
)
EXCLUDED_BINARIES = (
    "hubu-bench",
    "gongbu-sandbox",
)
RELEASE_TARGETS = (
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)
DEFERRED_LINUX_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
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

for binary in EXCLUDED_BINARIES:
    if f"--bin {binary}" in workflow_lines:
        fail(f"excluded binary {binary} must not be built for release")
    if re.search(rf'cp .*[/"]{re.escape(binary)}', package_script):
        fail(f"excluded binary {binary} must not be copied into release archives")

matrix_targets = re.findall(r"(?m)^\s+target: (\S+)$", workflow)
expected_matrix_targets = [*RELEASE_TARGETS, *RELEASE_TARGETS]
if matrix_targets != expected_matrix_targets:
    fail(
        "release build and published smoke matrices must each contain exactly "
        f"{RELEASE_TARGETS!r}; found {matrix_targets!r}"
    )
for target in DEFERRED_LINUX_TARGETS:
    if target in workflow:
        fail(f"deferred pre-launch Linux target remains in release workflow: {target}")
for target in RELEASE_TARGETS:
    asset = f'hubu-${{version}}-{target}.tar.gz'
    if workflow.count(asset) != 1:
        fail(f"complete-canary validation must require exactly one {target} asset")

for required in (
    "channel:",
    "default: canary",
    'INPUT_CHANNEL: ${{ inputs.channel }}',
    '"${INPUT_CHANNEL}" == "canary"',
    'git checkout --detach "${source_commit}"',
    "canary_release_is_complete",
    'gh release view "${tag}"',
    '.isDraft == false',
    '.isPrerelease == true',
    '.targetCommitish == $source_commit',
    '.state == "uploaded" and .size > 0',
    "GONGBU_PRODUCT_VERSION:",
    "GONGBU_SOURCE_COMMIT:",
    "HUBU_PRODUCT_VERSION:",
    "HUBU_SOURCE_COMMIT:",
    "./scripts/test-release-packaging.sh",
    "./scripts/test-release-archive-runtime.sh",
    "./scripts/release-smoke-test.sh",
    "hubu-spend-executor-v4.2",
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
    "validated immutable release workflow: four production binaries, exactly two "
    "temporary pre-launch macOS targets, scheduled and explicit canaries, shared "
    "build identity, bounded permissions, and pinned actions"
)
