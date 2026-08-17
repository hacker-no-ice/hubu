#!/usr/bin/env python3
"""Fail if ordinary CI can opt into credentialed or billable provider traffic."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/ci.yml"
LIVE_TEST_ROOTS = (ROOT / "crates/gongbu-api/src",)


def fail(message: str) -> None:
    print(f"CI provider safety check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


workflow = WORKFLOW.read_text(encoding="utf-8")

required_mock_settings = {
    'GONGBU_MCP_INTEGRATION: "0"',
    "GONGBU_SANDBOX_HUBU_MODE: mock",
    "GONGBU_SANDBOX_PROVIDER_MODE: mock",
}
for setting in sorted(required_mock_settings):
    if setting not in workflow:
        fail(f"{WORKFLOW.relative_to(ROOT)} is missing {setting!r}")

for forbidden in (
    "secrets.",
    "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND",
    "I_ACCEPT_GOOGLE_CHARGES",
    "I_ACCEPT_GOOGLE_AI_STUDIO_CHARGES",
    "GONGBU_LIVE_",
    "GONGBU_PROVIDER_CONFIG",
    "GONGBU_PRICING_CATALOG",
):
    if forbidden in workflow:
        fail(f"{WORKFLOW.relative_to(ROOT)} contains forbidden live-CI marker {forbidden!r}")

if not re.search(r"(?m)^permissions:\n  contents: read$", workflow):
    fail("workflow-wide permissions must remain contents: read")
if re.search(r"(?m)^\s+[a-z-]+: write\s*$", workflow):
    fail("ordinary CI must not request write permissions")

unpinned = []
for line_number, line in enumerate(workflow.splitlines(), start=1):
    match = re.search(r"\buses:\s*[^\s@]+@([^\s#]+)", line)
    if match and not re.fullmatch(r"[0-9a-f]{40}", match.group(1)):
        unpinned.append(f"line {line_number}: {line.strip()}")
if unpinned:
    fail("actions must be pinned to full commit SHAs: " + "; ".join(unpinned))

live_tests = []
unguarded_tests = []
for source_root in LIVE_TEST_ROOTS:
    for source in source_root.rglob("*.rs"):
        lines = source.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if re.search(r"\bfn live_[A-Za-z0-9_]*\s*\(", line):
                location = f"{source.relative_to(ROOT)}:{index + 1}"
                live_tests.append(location)
                attributes = "\n".join(lines[max(0, index - 4) : index])
                if "#[ignore" not in attributes:
                    unguarded_tests.append(location)

if not live_tests:
    fail("no live provider tests were found; update this guard if their naming changed")
if unguarded_tests:
    fail("live provider tests must remain ignored by default: " + ", ".join(unguarded_tests))

print(
    "validated ordinary CI is read-only, secret-free, mock-only, action-pinned, "
    f"and excludes {len(live_tests)} live provider tests"
)
