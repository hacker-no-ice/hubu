#!/usr/bin/env python3
"""Fail if ordinary CI can opt into credentialed or billable provider traffic."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/ci.yml"
ADVISORY_WORKFLOW = ROOT / ".github/workflows/security-advisories.yml"
LIVE_TEST_ROOTS = (ROOT / "crates/gongbu-api/src",)


def fail(message: str) -> None:
    print(f"CI provider safety check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


required_mock_settings = {
    'GONGBU_MCP_INTEGRATION: "0"',
    "GONGBU_SANDBOX_HUBU_MODE: mock",
    "GONGBU_SANDBOX_PROVIDER_MODE: mock",
}
forbidden_provider_markers = (
    "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND",
    "I_ACCEPT_GOOGLE_CHARGES",
    "I_ACCEPT_GOOGLE_AI_STUDIO_CHARGES",
    "GONGBU_LIVE_",
    "GONGBU_PROVIDER_CONFIG",
    "GONGBU_PRICING_CATALOG",
)


def check_provider_safety(path: Path, *, publisher: bool = False) -> None:
    workflow = path.read_text(encoding="utf-8")
    location = path.relative_to(ROOT)
    for setting in sorted(required_mock_settings):
        if setting not in workflow:
            fail(f"{location} is missing {setting!r}")
        key, expected = setting.split(": ", 1)
        for configured in re.findall(rf"(?m)^\s*{key}:\s*(.+)$", workflow):
            if configured != expected:
                fail(f"{location} overrides required mock setting {key}")
    for forbidden in forbidden_provider_markers:
        if forbidden in workflow:
            fail(f"{location} contains forbidden live-CI marker {forbidden!r}")
    if not re.search(r"(?m)^permissions:\n  contents: read$", workflow):
        fail(f"{location} workflow-wide permissions must remain contents: read")

    # Only the monitor's publication job may use a repository credential or
    # write permissions. Dependency resolution and validation stay unprivileged.
    read_only_content = workflow
    if publisher:
        if workflow.count("\njobs:\n") != 1:
            fail(f"{location} must have exactly one jobs block")
        read_only_content, job_text = workflow.split("\njobs:\n", 1)
        jobs = list(re.finditer(r"(?m)^  ([a-z][a-z0-9_-]*):\s*$", job_text))
        for index, match in enumerate(jobs):
            end = jobs[index + 1].start() if index + 1 < len(jobs) else len(job_text)
            if match.group(1) != "publish":
                read_only_content += job_text[match.start() : end]
    credential_access = r"\bsecrets\s*(?:\.|\[)|\bgithub\s*(?:\.\s*token\b|\[\s*['\"]token['\"]\s*\])"
    if re.search(credential_access, read_only_content):
        fail(f"{location} scan, dependency resolution, and tests must remain secret-free")
    if re.search(r"(?m)^\s+[a-z-]+: write\s*$", read_only_content):
        fail(f"{location} may grant write permissions only to advisory publication")

    unpinned = []
    for line_number, line in enumerate(workflow.splitlines(), start=1):
        match = re.search(r"\buses:\s*[^\s@]+@([^\s#]+)", line)
        if match and not re.fullmatch(r"[0-9a-f]{40}", match.group(1)):
            unpinned.append(f"line {line_number}: {line.strip()}")
    if unpinned:
        fail(f"{location} actions must be pinned to full commit SHAs: " + "; ".join(unpinned))


check_provider_safety(WORKFLOW)
check_provider_safety(ADVISORY_WORKFLOW, publisher=True)

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
    "validated ordinary CI and advisory scanning are secret-free, mock-only, "
    "action-pinned, restrict writes to advisory publication, "
    f"and exclude {len(live_tests)} live provider tests"
)
