#!/usr/bin/env python3
"""Check the trusted advisory monitor's scheduling and privilege boundaries."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/security-advisories.yml"
CI_WORKFLOW = ROOT / ".github/workflows/ci.yml"
INSTALL_ACTION = ROOT / ".github/actions/install-cargo-deny/action.yml"


def fail(message: str) -> None:
    print(f"security advisory workflow check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def block(source: str, key: str, indentation: int = 0) -> str:
    """Read an explicitly indented mapping block; unsupported forms fail closed."""
    lines = source.splitlines()
    heading = " " * indentation + key + ":"
    starts = [index for index, line in enumerate(lines) if line == heading]
    if len(starts) != 1:
        fail(f"expected exactly one {heading.strip()!r} block")
    content = []
    for line in lines[starts[0] + 1 :]:
        if line.strip() and not line.lstrip().startswith("#"):
            if len(line) - len(line.lstrip()) <= indentation:
                break
        content.append(line)
    return "\n".join(content)


def mapping(source: str, indentation: int) -> dict:
    values = {}
    for line in source.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if len(line) - len(line.lstrip()) != indentation:
            continue
        match = re.fullmatch(r"\s*([a-z][a-z0-9_-]*):(?:\s+(.*))?", line)
        if not match or match.group(1) in values:
            fail(f"unsupported or duplicate workflow mapping entry {line!r}")
        values[match.group(1)] = (match.group(2) or "").strip()
    return values


workflow = WORKFLOW.read_text(encoding="utf-8")
ci_workflow = CI_WORKFLOW.read_text(encoding="utf-8")
install_action = INSTALL_ACTION.read_text(encoding="utf-8")

triggers = mapping(block(workflow, "on"), 2)
if set(triggers) != {"schedule", "workflow_dispatch"}:
    fail("the monitor must accept only scheduled and manual runs")
if not re.search(r"(?m)^\s+- cron: [\"'][^\n]*\*/6[^\n]*[\"']$", workflow):
    fail("the monitor must retain its six-hour schedule")
if mapping(block(workflow, "permissions"), 2) != {"contents": "read"}:
    fail("workflow-wide permissions must be limited to contents: read")

jobs_source = block(workflow, "jobs")
job_names = set(mapping(jobs_source, 2))
if job_names != {"scan", "publish"}:
    fail(f"review the privilege boundary before changing monitor jobs: {job_names!r}")
for name in sorted(job_names):
    job = block(jobs_source, name, 2)
    condition = re.search(r"(?m)^    if:\s*(.*(?:\n      .*)*)", job)
    if condition is None:
        fail(f"{name} must explicitly require the trusted default branch")
    normalized = re.sub(r"\s+", " ", condition.group(1))
    trusted_guard = "github.ref == format('refs/heads/{0}', github.event.repository.default_branch)"
    expected_condition = trusted_guard if name == "scan" else f"always() && !cancelled() && {trusted_guard}"
    if normalized != expected_condition:
        fail(f"{name} must retain its trusted default-branch condition")

publish = block(jobs_source, "publish", 2)
scan = block(jobs_source, "scan", 2)
if "permissions" in mapping(scan, 4):
    if mapping(block(scan, "permissions", 4), 6) != {"contents": "read"}:
        fail("dependency scanning and validation may request only contents: read")
expected_publish_permissions = {
    "contents": "write",
    "pull-requests": "write",
    "issues": "write",
    "actions": "write",
}
if mapping(block(publish, "permissions", 4), 6) != expected_publish_permissions:
    fail("only publication may receive the reviewed repository-write permissions")
if re.search(r"(?m)^\s*(?:run: ?)?cargo\s", publish):
    fail("publication must not resolve dependencies, build, or run Rust code")
publisher_commands = re.findall(r"(?m)^\s+run:\s*(.+)$", publish)
if publisher_commands != ["bash scripts/security-advisories-push.sh"]:
    fail("publication may execute only the reviewed lockfile publication script")
if "scripts/security-advisories-github.js" not in publish:
    fail("publication must use the trusted metadata publisher")
if mapping(publish, 4).get("needs") != "scan":
    fail("publication must depend on the trusted scan job")

for action_source, label in ((workflow, "monitor"), (install_action, "cargo-deny installer")):
    for match in re.finditer(r"(?m)\buses:\s*([^\s@]+)@([^\s#]+)", action_source):
        if not re.fullmatch(r"[0-9a-f]{40}", match.group(2)):
            fail(f"{label} must pin {match.group(1)} to a full commit SHA")

# This action's Dockerfile ships cargo-deny 0.20.2. Review the publisher's
# Dockerfile and update this pair together when changing either tool version.
deny_action = "EmbarkStudios/cargo-deny-action@3c6349835b2b7b196a839186cb8b78e02f7b5f25"
if deny_action not in workflow or deny_action not in ci_workflow:
    fail("CI and scheduled policy checks must use the same reviewed cargo-deny action")
if not re.search(r"cargo install cargo-deny --version ['\"]=?0\.20\.2['\"] --locked", install_action):
    fail("the structured scanner must match the cargo-deny action's tool version")

for required in (
    "scripts/security-advisories.py",
    "cargo +1.88.0 check --workspace --all-targets --all-features --locked",
    "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings",
    "cargo test --workspace --all-targets --locked",
    "cargo build --release --locked",
    "advisories licenses sources",
):
    if required not in workflow:
        fail(f"monitor is missing required remediation validation {required!r}")
for binary in ("hubu", "hubu-server", "hubu-unified-mcp", "gongbu-server"):
    if not re.search(rf"--bin {re.escape(binary)}(?:\s|$)", workflow):
        fail(f"monitor must validate production binary {binary}")
if "python3 scripts/check-security-advisory-workflow.py" not in ci_workflow:
    fail("ordinary CI must check the advisory workflow boundary")

print("validated scheduled advisory detection, trusted-main publication, and remediation checks")
