#!/usr/bin/env python3
"""Summarize cargo-deny JSON and attempt conservative Cargo.lock-only fixes.

Requires Python 3.9+ and no third-party packages. Reports always distinguish
denied advisories from scanner/infrastructure errors. Exit zero means the report
was written; consumers must inspect its status before publishing a fix.
"""

import argparse
import copy
import hashlib
import json
import operator
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from functools import total_ordering
from pathlib import Path


REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
ADVISORY_CODES = {"vulnerability", "unmaintained", "unsound", "notice"}
INFRASTRUCTURE_CODES = {"index-failure", "index-cache-load-failure"}
PACKAGE_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*\Z")
VERSION_RE = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z"
)


class ReportError(Exception):
    """An incomplete scan or unsupported automatic remediation."""


@total_ordering
@dataclass(frozen=True)
class Version:
    major: int
    minor: int
    patch: int
    prerelease: tuple = ()

    @classmethod
    def parse(cls, value):
        match = VERSION_RE.fullmatch(value)
        if not match:
            raise ReportError("unsupported semantic version: " + str(value))
        prerelease = tuple(match[4].split(".")) if match[4] else ()
        if any(item.isdigit() and len(item) > 1 and item[0] == "0" for item in prerelease):
            raise ReportError("invalid numeric prerelease identifier: " + value)
        return cls(int(match[1]), int(match[2]), int(match[3]), prerelease)

    def sort_key(self):
        identifiers = tuple((0, int(item)) if item.isdigit() else (1, item) for item in self.prerelease)
        return (self.major, self.minor, self.patch, not self.prerelease, identifiers)

    def __lt__(self, other):
        return self.sort_key() < other.sort_key()


def compatible(current, candidate):
    """Cargo's caret compatibility, further restricted by cargo update itself."""
    if current.prerelease or candidate.prerelease or candidate <= current:
        return False
    if current.major:
        return candidate.major == current.major
    if current.minor:
        return (candidate.major, candidate.minor) == (0, current.minor)
    return (candidate.major, candidate.minor, candidate.patch) == (0, 0, current.patch)


def requirement(requirement_text):
    """Parse an explicit comparator conjunction, failing closed on other syntax.

    RustSec serializes a list of alternative patched version requirements.
    Support full semver comparator bounds (including prereleases), never infer
    meaning for caret, tilde, wildcard or partial-version requirements.
    """
    comparators = []
    comparisons = {">=": operator.ge, ">": operator.gt, "<=": operator.le, "<": operator.lt, "=": operator.eq}
    for part in requirement_text.split(","):
        match = re.fullmatch(r"\s*(>=|<=|>|<|=)\s*(\S+)\s*", part)
        if not match:
            raise ReportError("unsupported patched version requirement: " + requirement_text)
        comparators.append((comparisons[match[1]], Version.parse(match[2])))
    return lambda candidate: all(compare(candidate, bound) for compare, bound in comparators)


def key_for(advisory_id, package, version):
    identity = json.dumps([advisory_id, package, version], separators=(",", ":"))
    return hashlib.sha256(identity.encode()).hexdigest()[:24]


def empty_report(status="healthy"):
    return {"schema_version": 1, "status": status, "advisories": [], "updates": [], "errors": []}


def dependency_paths(graph):
    paths = []

    def walk(node, path, depth=0):
        if depth > 100:
            raise ReportError("cargo-deny dependency graph exceeds 100 levels")
        if not isinstance(node, dict):
            raise ReportError("unsupported cargo-deny dependency graph node")
        krate = node.get("Krate")
        if not isinstance(krate, dict) or not isinstance(krate.get("name"), str) or not isinstance(krate.get("version"), str):
            raise ReportError("unsupported cargo-deny dependency graph")
        label = krate["name"] + "@" + krate["version"]
        if krate.get("kind"):
            label += " (" + krate["kind"] + ")"
        next_path = path + [label]
        parents = node.get("parents", [])
        if not isinstance(parents, list):
            raise ReportError("invalid cargo-deny dependency graph parents")
        if parents:
            for parent in parents:
                walk(parent, next_path, depth + 1)
        else:
            text = " → ".join(reversed(next_path))
            if node.get("repeat"):
                text += " (repeated; see other path)"
            paths.append(text)

    walk(graph, [])
    return sorted(set(paths))


def patched_versions(fields):
    # cargo-deny 0.20.2 puts advisory metadata (without versions) in `advisory`;
    # the precise disjunction is emitted in the structured diagnostic's notes.
    solutions = [note for note in fields.get("notes", []) if isinstance(note, str) and note.startswith("Solution: ")]
    if solutions == ["Solution: No safe upgrade is available!"]:
        return []
    if len(solutions) != 1:
        raise ReportError("missing or ambiguous cargo-deny solution diagnostic")
    match = re.fullmatch(r"Solution: Upgrade to (.+) \(try `cargo update -p [A-Za-z0-9_-]+`\)", solutions[0])
    if not match:
        raise ReportError("unsupported cargo-deny solution diagnostic")
    return match[1].split(" OR ")


def parse_scan(diagnostics, exit_code, baseline=None):
    report = empty_report()
    summary_errors = None
    found_errors = 0
    for number, line in enumerate(diagnostics.splitlines(), 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
            if not isinstance(record, dict):
                raise ValueError("cargo-deny record must be an object")
            fields = record["fields"]
            record_type = record["type"]
            if not isinstance(fields, dict):
                raise ValueError("fields must be an object")
            if record_type == "log":
                if str(fields.get("level", "")).lower() == "error":
                    report["errors"].append(str(fields.get("message", "cargo-deny error log")))
                continue
            if record_type == "summary":
                if "advisories" in fields:
                    summary_errors = fields["advisories"]["errors"]
                    if not isinstance(summary_errors, int) or summary_errors < 0:
                        raise ValueError("invalid advisory summary")
                continue
            if record_type != "diagnostic":
                raise ValueError("unknown cargo-deny record type")
            code = fields.get("code")
            if code in INFRASTRUCTURE_CODES:
                report["errors"].append(str(fields.get("message", code)))
            if fields.get("severity") not in {"error", "bug"}:
                continue
            found_errors += 1
            if code not in ADVISORY_CODES:
                report["errors"].append("cargo-deny " + str(code) + ": " + str(fields.get("message", "policy error")))
                continue
            advisory = fields["advisory"]
            if not isinstance(advisory, dict):
                raise ValueError("advisory metadata must be an object")
            advisory_id = advisory["id"]
            package = advisory["package"]
            if not isinstance(advisory_id, str) or not re.fullmatch(r"[A-Za-z0-9]+(?:-[A-Za-z0-9]+)+", advisory_id):
                raise ValueError("invalid advisory ID")
            if not isinstance(package, str) or not PACKAGE_RE.fullmatch(package):
                raise ValueError("invalid advisory package")
            graphs = fields["graphs"]
            if not isinstance(graphs, list) or not graphs:
                raise ValueError("missing advisory dependency graph")
            patched = patched_versions(fields)
            for graph in graphs:
                if not isinstance(graph, dict) or not isinstance(graph.get("Krate"), dict):
                    raise ValueError("invalid advisory dependency graph root")
                root = graph["Krate"]
                if root["name"] != package:
                    raise ValueError("advisory package differs from graph root")
                version = root["version"]
                Version.parse(version)
                source = advisory.get("source") or REGISTRY
                item = {
                    "id": advisory_id, "title": advisory.get("title", fields.get("message", advisory_id)),
                    "package": package, "version": version,
                    "url": "https://rustsec.org/advisories/" + advisory_id + ".html" if advisory_id.startswith("RUSTSEC-") else advisory.get("url"),
                    "patched_versions": patched, "dependency_paths": dependency_paths(graph),
                    "key": key_for(advisory_id, package, version), "classification": "unclassified", "source": source,
                }
                existing = next((entry for entry in report["advisories"] if entry["key"] == item["key"]), None)
                if existing:
                    existing["dependency_paths"] = sorted(set(existing["dependency_paths"] + item["dependency_paths"]))
                else:
                    report["advisories"].append(item)
        except (KeyError, TypeError, ValueError, ReportError) as error:
            report["errors"].append("diagnostic line " + str(number) + ": " + str(error))

    if summary_errors is None:
        report["errors"].append("cargo-deny did not emit an advisories summary; scan completeness is unknown")
    elif summary_errors != found_errors:
        report["errors"].append("cargo-deny advisory error summary does not match parsed diagnostics")
    if exit_code not in (0, 1):
        report["errors"].append("cargo-deny exited with unexpected status " + str(exit_code))
    elif bool(exit_code) != bool(found_errors):
        report["errors"].append("cargo-deny exit status does not match its diagnostics")
    report["status"] = "error" if report["errors"] else ("advisories" if report["advisories"] else "healthy")
    report["advisories"].sort(key=lambda item: (item["id"], item["package"], item["version"]))
    if baseline is not None:
        valid_baseline = (isinstance(baseline, dict) and baseline.get("schema_version") == 1
                          and baseline.get("status") in {"healthy", "advisories"}
                          and not baseline.get("errors") and isinstance(baseline.get("advisories"), list)
                          and all(isinstance(item, dict) and isinstance(item.get("key"), str) for item in baseline["advisories"]))
        if not valid_baseline:
            report["baseline_status"] = "error"
            report["message"] = "main baseline failed; advisory classification is unknown"
            for item in report["advisories"]:
                item["classification"] = "unknown"
        else:
            report["baseline_status"] = baseline["status"]
            existing_keys = {item["key"] for item in baseline.get("advisories", [])}
            for item in report["advisories"]:
                item["classification"] = "pre-existing" if item["key"] in existing_keys else "introduced"
    return report


def fetch_versions(package):
    if not PACKAGE_RE.fullmatch(package):
        raise ReportError("invalid crates.io package name")
    name = package.lower()
    if len(name) <= 2:
        suffix = str(len(name)) + "/" + name
    elif len(name) == 3:
        suffix = "3/" + name[0] + "/" + name
    else:
        suffix = name[:2] + "/" + name[2:4] + "/" + name
    request = urllib.request.Request("https://index.crates.io/" + suffix, headers={"User-Agent": "hubu-security-advisories/1.0"})
    with urllib.request.urlopen(request, timeout=30) as response:
        data = response.read(16 * 1024 * 1024 + 1)
    if len(data) > 16 * 1024 * 1024:
        raise ReportError("crates.io index response exceeds size limit")
    versions = [json.loads(line) for line in data.decode().splitlines() if line]
    if not versions or any(not isinstance(item, dict) or not isinstance(item.get("name"), str)
                           or item["name"].lower() != name for item in versions):
        raise ReportError("crates.io index contains a different package name")
    return versions


def msrv_allowed(value, msrv):
    if value is None:
        return True
    if not re.fullmatch(r"\d+\.\d+(?:\.\d+)?", value):
        return False
    parts = tuple(int(part) for part in value.split("."))
    return parts + (0,) * (3 - len(parts)) <= msrv


def candidate_versions(advisories, versions, msrv=(1, 88, 0)):
    current = Version.parse(advisories[0]["version"])
    checks = []
    for advisory in advisories:
        patched = advisory["patched_versions"]
        if not patched:
            raise ReportError(advisory["id"] + " has no published safe upgrade")
        checks.append([requirement(text) for text in patched])
    candidates = []
    for record in versions:
        if record.get("yanked") or not msrv_allowed(record.get("rust_version"), msrv):
            continue
        parsed = Version.parse(record["vers"])
        if compatible(current, parsed) and all(any(check(parsed) for check in alternatives) for alternatives in checks):
            candidates.append((parsed, record["vers"]))
    return [value for _, value in sorted(set(candidates))]


def lock_packages(lock_text):
    """Read only Cargo's machine-generated package identity triples, not TOML."""
    result = set()
    for block in re.split(r"(?m)^\[\[package\]\]\s*$", lock_text)[1:]:
        fields = {}
        for key in ("name", "version", "source"):
            match = re.search(r'(?m)^' + key + r'\s*=\s*("[^"\n]*")\s*$', block)
            if match:
                fields[key] = json.loads(match[1])
        if "name" not in fields or "version" not in fields:
            raise ReportError("Cargo.lock package identity is malformed")
        result.add((fields["name"], fields["version"], fields.get("source")))
    if not result:
        raise ReportError("Cargo.lock has no package records")
    return result


def remediate(scan, cwd, fetch=fetch_versions, run=subprocess.run):
    report = copy.deepcopy(scan)
    report["updates"] = []
    if scan.get("status") == "healthy":
        return report
    if scan.get("status") != "advisories":
        report["status"] = "error"
        report["errors"].append("automatic remediation requires a complete denied-advisory scan")
        return report
    lockfile = Path(cwd) / "Cargo.lock"
    original = lockfile.read_bytes()
    groups = {}
    for advisory in scan["advisories"]:
        groups.setdefault((advisory["package"], advisory["version"]), []).append(advisory)
    blockers = []
    try:
        original_packages = lock_packages(original.decode())
        for (package, old), advisories in sorted(groups.items()):
            no_patch = [item["id"] for item in advisories if not item.get("patched_versions")]
            if no_patch:
                blockers.append(package + "@" + old + ": " + ", ".join(no_patch) + " has no published safe upgrade")
                continue
            if any(item.get("source") != REGISTRY for item in advisories):
                blockers.append(package + "@" + old + ": automatic fixes support crates.io dependencies only")
                continue
            before = lockfile.read_bytes()
            if (package, old, REGISTRY) not in original_packages:
                blockers.append(package + "@" + old + ": lockfile no longer matches the scan; rescan required")
                continue
            current_packages = lock_packages(before.decode())
            if (package, old, REGISTRY) not in current_packages:
                # A preceding precise update can resolve another vulnerable
                # package transitively. Recognize one unambiguous safe current
                # version instead of retrying a package spec that no longer exists.
                current_versions = [{"vers": version} for name, version, source in current_packages if name == package and source == REGISTRY]
                replacements = candidate_versions(advisories, current_versions)
                if len(replacements) == 1:
                    report["updates"].append({"package": package, "from": old, "to": replacements[0]})
                    continue
                blockers.append(package + "@" + old + ": a preceding update removed this version without one unambiguous safe replacement; rescan required")
                continue
            try:
                candidates = candidate_versions(advisories, fetch(package))
            except (ReportError, KeyError, ValueError) as error:
                blockers.append(package + "@" + old + ": " + str(error))
                continue
            if not candidates:
                blockers.append(package + "@" + old + ": no non-yanked stable semver-compatible fix supports Rust 1.88")
                continue
            failure = ""
            updated = False
            # Bound resolver work. An unresolved range is left for the maintainer.
            for target in candidates[:10]:
                lockfile.write_bytes(before)
                result = run(["cargo", "update", "--package", package + "@" + old, "--precise", target], cwd=str(cwd), text=True, capture_output=True, timeout=180)
                if result.returncode:
                    failure = (result.stderr or result.stdout or "Cargo resolver failed")[-4000:]
                    continue
                packages = lock_packages(lockfile.read_text())
                if (package, target, REGISTRY) not in packages or (package, old, REGISTRY) in packages:
                    failure = "Cargo did not replace the affected package with the selected precise version"
                    continue
                report["updates"].append({"package": package, "from": old, "to": target})
                updated = True
                break
            if not updated:
                lockfile.write_bytes(before)
                blockers.append(package + "@" + old + ": no attempted precise update resolved within existing manifest constraints. " + failure)
        if blockers:
            report["status"] = "blocked"
            report["errors"].extend(blockers)
            report["updates"] = []
            lockfile.write_bytes(original)
        elif report["updates"]:
            report["status"] = "fix_available"
            report["message"] = "Targeted lockfile updates prepared; full policy, MSRV and workspace validation must pass before publication."
        else:
            raise ReportError("no affected packages were available for remediation")
    except (OSError, ValueError, KeyError, ReportError, subprocess.SubprocessError, urllib.error.URLError) as error:
        lockfile.write_bytes(original)
        report["status"] = "error"
        report["updates"] = []
        report["errors"].append("remediation failed: " + str(error))
    return report


def markdown_text(value):
    return str(value).replace("\n", " ").replace("\r", " ").replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("|", "\\|").replace("`", "'")


def markdown(report):
    lines = ["## Rust security advisories", "", "Status: **" + report["status"] + "**.", ""]
    if report.get("message"):
        lines.extend([markdown_text(report["message"]), ""])
    if report.get("baseline_status") in {"healthy", "advisories"} and report["advisories"]:
        existing = sum(item.get("classification") == "pre-existing" for item in report["advisories"])
        introduced = sum(item.get("classification") == "introduced" for item in report["advisories"])
        if existing == len(report["advisories"]):
            lines.extend(["All denied advisories already affect main at the same package versions. The required security gate remains blocking.", ""])
        elif not existing:
            lines.extend(["All denied advisory/package/version combinations are introduced relative to main. The required security gate remains blocking.", ""])
        else:
            lines.extend([str(existing) + " denied advisory/package/version combinations already affect main; " + str(introduced) + " are introduced relative to main. The required security gate remains blocking.", ""])
    repository = os.environ.get("GITHUB_REPOSITORY", "hacker-no-ice/hubu")
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        lines.extend(["See the [security maintenance workflow](https://github.com/" + repository + "/actions/workflows/security-advisories.yml) for remediation tracking and validation evidence.", ""])
    for item in report["advisories"]:
        lines.extend(["### " + markdown_text(item["id"]) + " · " + markdown_text(item["package"]) + "@" + markdown_text(item["version"]), "", markdown_text(item["title"]), ""])
        if item["id"].startswith("RUSTSEC-"):
            lines.extend(["[RustSec advisory](https://rustsec.org/advisories/" + item["id"] + ".html)", ""])
        if item.get("classification", "unclassified") != "unclassified":
            lines.extend(["Compared with main: **" + markdown_text(item["classification"]) + "**.", ""])
        lines.extend(["Patched versions: " + (" OR ".join(markdown_text(text) for text in item["patched_versions"]) or "No published safe upgrade."), "", "Dependency paths:", ""])
        lines.extend("- " + markdown_text(path) for path in item["dependency_paths"])
        lines.append("")
    if report["updates"]:
        lines.extend(["### Targeted updates", ""])
        lines.extend("- " + markdown_text(item["package"]) + ": " + markdown_text(item["from"]) + " → " + markdown_text(item["to"]) for item in report["updates"])
        lines.append("")
    if report["errors"]:
        lines.extend(["### Action required", ""])
        lines.extend("- " + markdown_text(error) for error in report["errors"])
        lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    scan = commands.add_parser("scan")
    scan.add_argument("--diagnostics", type=Path)
    scan.add_argument("--exit-code", type=int)
    scan.add_argument("--run", action="store_true")
    scan.add_argument("--offline", action="store_true")
    scan.add_argument("--baseline", type=Path)
    fix = commands.add_parser("remediate")
    fix.add_argument("--report", type=Path, required=True)
    for command in (scan, fix):
        command.add_argument("--cwd", type=Path, default=Path.cwd())
        command.add_argument("--output", type=Path, required=True)
        command.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "scan":
            if args.run:
                command = ["cargo", "deny", "--format", "json", "--all-features", "--locked"]
                if args.offline:
                    command.append("--offline")
                command.extend(["check", "advisories"])
                result = subprocess.run(command, cwd=str(args.cwd), text=True, capture_output=True, timeout=480)
                diagnostics = result.stderr + result.stdout
                if args.diagnostics:
                    args.diagnostics.parent.mkdir(parents=True, exist_ok=True)
                    args.diagnostics.write_text(diagnostics)
                exit_code = result.returncode
            else:
                if args.diagnostics is None or args.exit_code is None:
                    parser.error("scan requires --run or both --diagnostics and --exit-code")
                diagnostics, exit_code = args.diagnostics.read_text(), args.exit_code
            baseline = json.loads(args.baseline.read_text()) if args.baseline else None
            report = parse_scan(diagnostics, exit_code, baseline)
        else:
            report = remediate(json.loads(args.report.read_text()), args.cwd)
    except (OSError, ValueError, KeyError, TypeError, ReportError, subprocess.SubprocessError) as error:
        report = empty_report("error")
        report["errors"].append(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    args.markdown.parent.mkdir(parents=True, exist_ok=True)
    args.markdown.write_text(markdown(report) + "\n")
    print(report["status"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
