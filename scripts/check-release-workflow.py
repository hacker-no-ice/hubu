#!/usr/bin/env python3
"""Validate immutable unified-release coverage without publishing anything."""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/release.yml"
PACKAGE_SCRIPT = ROOT / "scripts/package-release-archive.sh"
SMOKE_SCRIPT = ROOT / "scripts/verify-release-archive.sh"
CHANGELOG = ROOT / "CHANGELOG.md"
RELEASE_DOC = ROOT / "docs/operations/releases.md"

PRODUCTION_PACKAGE_MANIFESTS = {
    "gongbu-api": ROOT / "crates/gongbu-api/Cargo.toml",
    "hubu-api": ROOT / "crates/hubu-api/Cargo.toml",
    "hubu-cli": ROOT / "crates/hubu-cli/Cargo.toml",
    "hubu-unified-mcp": ROOT / "crates/hubu-unified-mcp/Cargo.toml",
}

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
changelog = CHANGELOG.read_text(encoding="utf-8")
release_doc = RELEASE_DOC.read_text(encoding="utf-8")
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
    "- candidate",
    'INPUT_CHANNEL: ${{ inputs.channel }}',
    '"${INPUT_CHANNEL}" == "canary"',
    '"${INPUT_CHANNEL}" == "candidate"',
    "Candidate version must match vMAJOR.MINOR.PATCH-rc.NUMBER",
    'candidate_base_version="${INPUT_VERSION#v}"',
    'candidate_base_version="${candidate_base_version%-rc.*}"',
    '"${candidate_base_version}" != "${base_version}"',
    'stable_base_version="${INPUT_VERSION#v}"',
    '"${stable_base_version}" != "${base_version}"',
    "resolve_base_version",
    "Release production packages must share one source package version",
    'RELEASE_CHANNEL: ${{ needs.resolve.outputs.channel }}',
    'git checkout --detach "${source_commit}"',
    "prerelease_is_complete",
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
    "hubu-spend-executor-v4.3",
):
    if required not in workflow:
        fail(f"release workflow is missing {required!r}")

if workflow.count("cargo metadata --locked --no-deps --format-version 1") != 1:
    fail("release identity must resolve locked source package versions in one helper")
for package in PRODUCTION_PACKAGE_MANIFESTS:
    if workflow.count(f'.name == "{package}"') != 1:
        fail(f"release identity must include production package {package}")

changelog_parts = changelog.split("## Unreleased", 1)
if len(changelog_parts) != 2:
    fail("changelog must contain an Unreleased section before release history")
release_headings = re.findall(r"(?m)^## (.+)$", changelog_parts[1])
if not release_headings:
    fail("changelog must contain release history after the Unreleased section")
release_heading_pattern = re.compile(
    r"(v[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[1-9][0-9]*)?) — "
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}"
)
release_versions = []
for heading in release_headings:
    match = release_heading_pattern.fullmatch(heading)
    if match is None:
        fail(f"invalid changelog release heading {heading!r}")
    release_versions.append(match.group(1))
latest_release = release_versions[0]
changelog_base_version = latest_release.removeprefix("v").split("-rc.", 1)[0]
if "-rc." not in latest_release:
    retained_candidates = [
        version for version in release_versions[1:] if "-rc." in version
    ]
    if retained_candidates:
        fail(
            f"stable changelog {latest_release} must fold and remove candidate "
            f"history; found {retained_candidates!r}"
        )
package_versions = {}
for package, manifest in PRODUCTION_PACKAGE_MANIFESTS.items():
    package_manifest = manifest.read_text(encoding="utf-8")
    package_version = re.search(
        r'(?m)^version\s*=\s*"([^"]+)"\s*$', package_manifest
    )
    if package_version is None:
        fail(f"production package {package} has no explicit version")
    package_versions[package] = package_version.group(1)
source_versions = set(package_versions.values())
if len(source_versions) != 1:
    fail(f"production packages must share one version; found {package_versions!r}")
source_version = next(iter(source_versions))
if changelog_base_version != source_version:
    fail(
        f"latest changelog release {latest_release} does not match source "
        f"package version {source_version}"
    )
candidate_examples = re.findall(
    r"(?m)^\s+-f version="
    r"(v[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*)\s+\\\s*$",
    release_doc,
)
stable_examples = re.findall(
    r"(?m)^\s+-f version=(v[0-9]+\.[0-9]+\.[0-9]+)\s+\\\s*$",
    release_doc,
)
if len(candidate_examples) != 1:
    fail("candidate runbook must contain exactly one valid candidate version")
if len(stable_examples) != 1:
    fail("stable runbook must contain exactly one valid stable version")
candidate_example = candidate_examples[0]
candidate_base_version = candidate_example.removeprefix("v").split("-rc.", 1)[0]
if candidate_base_version != source_version:
    fail(
        f"candidate runbook version {candidate_example} does not match source "
        f"package version {source_version}"
    )
expected_stable = f"v{source_version}"
if stable_examples[0] != expected_stable:
    fail(f"stable runbook must select source package version {expected_stable}")
if "-rc." in latest_release and candidate_example != latest_release:
    fail(
        f"candidate runbook must select the active changelog candidate "
        f"{latest_release}"
    )

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
    "temporary pre-launch macOS targets, scheduled and explicit canaries, "
    "versioned candidates, source-version validation, synchronized release docs, "
    "shared build identity, bounded permissions, and pinned actions"
)
