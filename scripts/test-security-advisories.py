#!/usr/bin/env python3
"""Deterministic tests for HUB-289 diagnostics, baseline comparison and fixes."""

import copy
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch


SCRIPT = Path(__file__).with_name("security-advisories.py")
SPEC = importlib.util.spec_from_file_location("security_advisories", SCRIPT)
SECURITY = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SECURITY
SPEC.loader.exec_module(SECURITY)


# Reduced from cargo-deny 0.20.2 JSON diagnostics captured against Hubu on
# 2026-10-20. Metadata descriptions/aliases and duplicate graph branches are
# omitted; the real graph shape, solution grammar, labels and summary remain.
WEBPKI = {
    "type": "diagnostic",
    "fields": {
        "advisory": {
            "id": "RUSTSEC-2026-0381", "package": "webpki-roots", "source": None,
            "title": "Bundled root certificates allow trust in distrusted emSign intermediates",
            "url": "https://github.com/rustls/webpki-roots/pull/133",
        },
        "code": "vulnerability", "severity": "error",
        "message": "Bundled root certificates allow trust in distrusted emSign intermediates",
        "labels": [{"column": 1, "line": 317, "message": "security vulnerability detected", "span": "webpki-roots 1.0.9 registry+https://github.com/rust-lang/crates.io-index"}],
        "graphs": [{
            "Krate": {"name": "webpki-roots", "version": "1.0.9"},
            "parents": [{
                "Krate": {"name": "hyper-rustls", "version": "0.27.9"},
                "parents": [{
                    "Krate": {"name": "reqwest", "version": "0.12.28"},
                    "parents": [{"Krate": {"name": "gongbu-api", "version": "0.2.4"}}, {"Krate": {"name": "hubu-cli", "version": "0.2.4"}}],
                }],
            }, {"Krate": {"name": "reqwest", "version": "0.12.28"}, "repeat": True}],
        }],
        "notes": ["ID: RUSTSEC-2026-0381", "Advisory: https://rustsec.org/advisories/RUSTSEC-2026-0381", "Solution: Upgrade to >=1.0.10 (try `cargo update -p webpki-roots`)"],
    },
}
RUSTLS = {
    "type": "diagnostic",
    "fields": {
        "advisory": {
            "id": "RUSTSEC-2026-0382", "package": "rustls-webpki", "source": None,
            "title": "Certificate verification may accept distrusted intermediates",
            "url": "https://github.com/rustls/webpki/pull/546",
        },
        "code": "vulnerability", "severity": "error",
        "message": "Certificate verification may accept distrusted intermediates",
        "labels": [{"column": 1, "line": 216, "message": "security vulnerability detected", "span": "rustls-webpki 0.103.14 registry+https://github.com/rust-lang/crates.io-index"}],
        "graphs": [{"Krate": {"name": "rustls-webpki", "version": "0.103.14"}, "parents": [{"Krate": {"name": "rustls", "version": "0.23.46"}, "parents": [{"Krate": {"name": "hyper-rustls", "version": "0.27.9"}, "parents": [{"Krate": {"name": "reqwest", "version": "0.12.28"}, "parents": [{"Krate": {"name": "hubu-unified-mcp", "version": "0.2.4"}}]}]}]}]}],
        "notes": ["ID: RUSTSEC-2026-0382", "Advisory: https://rustsec.org/advisories/RUSTSEC-2026-0382", "Solution: Upgrade to >=0.103.16, <0.104.0-alpha.1 OR >=0.104.0-alpha.8 (try `cargo update -p rustls-webpki`)"],
    },
}
YANKED_WARNING = {
    "type": "diagnostic",
    "fields": {"code": "yanked", "severity": "warning", "message": "detected yanked crate (try `cargo update -p chacha20`)", "graphs": [{"Krate": {"name": "chacha20", "version": "0.10.1"}}]},
}


def diagnostics(*records, errors=0, warnings=0):
    summary = {"type": "summary", "fields": {"advisories": {"errors": errors, "helps": 0, "notes": 0, "warnings": warnings}}}
    return "\n".join(json.dumps(record) for record in records + (summary,)) + "\n"


def scanned(*records):
    return SECURITY.parse_scan(diagnostics(*records, errors=len(records)), 1 if records else 0)


def index_version(name, version, yanked=False, rust_version=None):
    return {"name": name, "vers": version, "yanked": yanked, "rust_version": rust_version}


def lockfile(*packages):
    return "version = 4\n\n" + "\n".join('[[package]]\nname = "' + name + '"\nversion = "' + version + '"\nsource = "' + SECURITY.REGISTRY + '"\n' for name, version in packages)


class ScanTests(unittest.TestCase):
    def test_real_denied_advisories_and_warning(self):
        report = SECURITY.parse_scan(diagnostics(WEBPKI, RUSTLS, YANKED_WARNING, errors=2, warnings=1), 1)
        self.assertEqual(report["status"], "advisories")
        self.assertEqual(len(report["advisories"]), 2)
        self.assertEqual(report["errors"], [])
        advisory = report["advisories"][0]
        self.assertEqual(advisory["patched_versions"], [">=1.0.10"])
        self.assertIn("gongbu-api@0.2.4 → reqwest@0.12.28 → hyper-rustls@0.27.9 → webpki-roots@1.0.9", advisory["dependency_paths"])
        self.assertTrue(any("repeated" in path for path in advisory["dependency_paths"]))
        self.assertRegex(advisory["key"], r"^[0-9a-f]{24}$")

    def test_warnings_do_not_trigger_remediation(self):
        report = SECURITY.parse_scan(diagnostics(YANKED_WARNING, warnings=1), 0)
        self.assertEqual(report["status"], "healthy")
        self.assertEqual(report["advisories"], [])

    def test_ignored_advisory_is_not_remediated(self):
        ignored = copy.deepcopy(WEBPKI)
        ignored["fields"]["severity"] = "note"
        report = SECURITY.parse_scan(diagnostics(ignored), 0)
        self.assertEqual(report["status"], "healthy")

    def test_missing_summary_cannot_be_clean(self):
        self.assertEqual(SECURITY.parse_scan("", 0)["status"], "error")

    def test_malformed_json_and_inconsistent_exit_fail_closed(self):
        self.assertEqual(SECURITY.parse_scan("not JSON\n" + diagnostics(), 0)["status"], "error")
        self.assertEqual(SECURITY.parse_scan(diagnostics(), 1)["status"], "error")
        self.assertEqual(SECURITY.parse_scan(diagnostics(WEBPKI, errors=1), 0)["status"], "error")
        self.assertEqual(SECURITY.parse_scan(diagnostics(), 101)["status"], "error")

    def test_database_failure_is_infrastructure(self):
        log = {"fields": {"level": "ERROR", "message": "failed to acquire advisory database lock"}, "type": "log"}
        report = SECURITY.parse_scan(json.dumps(log), 1)
        self.assertEqual(report["status"], "error")
        self.assertEqual(report["advisories"], [])
        self.assertIn("failed to acquire advisory database lock", report["errors"])

    def test_registry_index_warning_is_incomplete_scan(self):
        warning = {"type": "diagnostic", "fields": {"code": "index-failure", "severity": "warning", "message": "unable to check for yanked crates"}}
        self.assertEqual(SECURITY.parse_scan(diagnostics(warning, warnings=1), 0)["status"], "error")

    def test_unsupported_policy_error_not_reported_as_advisory(self):
        error = {"type": "diagnostic", "fields": {"code": "yanked", "severity": "error", "message": "yanked version"}}
        self.assertEqual(scanned(error)["status"], "error")

    def test_missing_solution_and_graph_fail_closed(self):
        for field in ("notes", "graphs", "advisory"):
            malformed = copy.deepcopy(WEBPKI)
            malformed["fields"].pop(field)
            self.assertEqual(scanned(malformed)["status"], "error", field)

    def test_summary_must_match_error_count(self):
        report = SECURITY.parse_scan(diagnostics(WEBPKI, errors=2), 1)
        self.assertEqual(report["status"], "error")

    def test_baseline_requires_same_advisory_package_and_version(self):
        baseline = scanned(WEBPKI)
        report = SECURITY.parse_scan(diagnostics(WEBPKI, RUSTLS, errors=2), 1, baseline)
        self.assertEqual([item["classification"] for item in report["advisories"]], ["pre-existing", "introduced"])
        different = copy.deepcopy(WEBPKI)
        different["fields"]["graphs"][0]["Krate"]["version"] = "1.0.8"
        report = SECURITY.parse_scan(diagnostics(different, errors=1), 1, baseline)
        self.assertEqual(report["advisories"][0]["classification"], "introduced")

    def test_failed_baseline_is_unknown_not_preexisting(self):
        report = SECURITY.parse_scan(diagnostics(WEBPKI, errors=1), 1, SECURITY.empty_report("error"))
        self.assertEqual(report["advisories"][0]["classification"], "unknown")

    def test_invalid_baseline_and_graph_are_handled_as_data(self):
        for baseline in ([], {"status": "healthy"}, {"schema_version": 1, "status": "healthy", "advisories": [None]}):
            report = SECURITY.parse_scan(diagnostics(WEBPKI, errors=1), 1, baseline)
            self.assertEqual(report["advisories"][0]["classification"], "unknown")
        invalid = copy.deepcopy(WEBPKI)
        invalid["fields"]["graphs"][0]["parents"] = [None]
        self.assertEqual(scanned(invalid)["status"], "error")

    def test_repeated_diagnostic_is_deduplicated(self):
        report = scanned(WEBPKI, WEBPKI)
        self.assertEqual(len(report["advisories"]), 1)

    def test_no_safe_upgrade_is_a_valid_advisory(self):
        notice = copy.deepcopy(WEBPKI)
        notice["fields"]["code"] = "unmaintained"
        notice["fields"]["notes"] = ["Solution: No safe upgrade is available!"]
        report = scanned(notice)
        self.assertEqual(report["status"], "advisories")
        self.assertEqual(report["advisories"][0]["patched_versions"], [])


class CandidateTests(unittest.TestCase):
    def test_lowest_stable_unyanked_msrv_compatible_fix(self):
        advisory = scanned(WEBPKI)["advisories"]
        versions = [index_version("webpki-roots", version) for version in ("1.0.11", "1.0.10", "1.0.9", "1.1.0-beta.1", "2.0.0")]
        versions += [index_version("webpki-roots", "1.0.12", yanked=True), index_version("webpki-roots", "1.0.13", rust_version="1.89")]
        self.assertEqual(SECURITY.candidate_versions(advisory, versions), ["1.0.10", "1.0.11"])

    def test_actual_prerelease_bounded_disjunction(self):
        advisory = scanned(RUSTLS)["advisories"]
        versions = [index_version("rustls-webpki", version) for version in ("0.103.15", "0.103.16", "0.103.17", "0.104.0-alpha.8", "0.104.0")]
        self.assertEqual(SECURITY.candidate_versions(advisory, versions), ["0.103.16", "0.103.17"])

    def test_intersection_of_multiple_advisories(self):
        advisories = scanned(WEBPKI)["advisories"]
        other = copy.deepcopy(advisories[0])
        other["id"], other["patched_versions"] = "RUSTSEC-2026-9999", [">=1.0.11, <1.1.0"]
        versions = [index_version("webpki-roots", version) for version in ("1.0.10", "1.0.11", "1.1.0")]
        self.assertEqual(SECURITY.candidate_versions(advisories + [other], versions), ["1.0.11"])

    def test_unsupported_requirements_are_never_guessed(self):
        for expression in ("^1.0.10", "~1.0.10", "1.*", ">=1.0", "1.0.10", "", ">=1.0.10 || >=2.0.0"):
            with self.assertRaises(SECURITY.ReportError, msg=expression):
                SECURITY.requirement(expression)

    def test_zero_major_caret_boundaries(self):
        for old, new, expected in (("0.1.0", "0.1.1", True), ("0.1.0", "0.2.0", False), ("0.0.1", "0.0.2", False), ("1.0.0", "1.2.0", True), ("1.0.0", "2.0.0", False)):
            self.assertEqual(SECURITY.compatible(SECURITY.Version.parse(old), SECURITY.Version.parse(new)), expected)

    def test_msrv_partial_and_full_versions(self):
        for version, expected in (("1.88", True), ("1.88.0", True), ("1.88.1", False), ("1.89", False), (None, True), ("nightly", False)):
            self.assertEqual(SECURITY.msrv_allowed(version, (1, 88, 0)), expected)

    def test_semver_prerelease_order_and_metadata(self):
        values = ["1.0.0-alpha.2", "1.0.0-alpha.11", "1.0.0-beta", "1.0.0"]
        self.assertEqual(sorted(reversed(values), key=SECURITY.Version.parse), values)
        self.assertEqual(SECURITY.Version.parse("1.0.0+build.1"), SECURITY.Version.parse("1.0.0"))
        with self.assertRaises(SECURITY.ReportError):
            SECURITY.Version.parse("1.0.0-alpha.01")

    def test_sparse_index_paths_and_response_identity(self):
        for name, suffix in (("a", "1/a"), ("ab", "2/ab"), ("abc", "3/a/abc"), ("webpki-roots", "we/bp/webpki-roots")):
            response = io.BytesIO((json.dumps(index_version(name, "1.0.10")) + "\n").encode())
            with patch.object(SECURITY.urllib.request, "urlopen", return_value=response) as opened:
                self.assertEqual(SECURITY.fetch_versions(name)[0]["name"], name)
                self.assertEqual(opened.call_args.args[0].full_url, "https://index.crates.io/" + suffix)
        with patch.object(SECURITY.urllib.request, "urlopen", return_value=io.BytesIO(b'{"name":"different","vers":"1.0.0"}\n')):
            with self.assertRaises(SECURITY.ReportError):
                SECURITY.fetch_versions("expected")


class RemediationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.cwd = Path(self.directory.name)
        self.lock = self.cwd / "Cargo.lock"
        self.original = lockfile(("webpki-roots", "1.0.9"))
        self.lock.write_text(self.original)
        self.report = scanned(WEBPKI)
        self.fetch = Mock(return_value=[index_version("webpki-roots", "1.0.11"), index_version("webpki-roots", "1.0.10")])

    def successful_update(self, argv, **kwargs):
        self.assertEqual(argv[:3], ["cargo", "update", "--package"])
        self.assertEqual(argv[3], "webpki-roots@1.0.9")
        self.assertEqual(argv[4], "--precise")
        self.assertEqual(kwargs["cwd"], str(self.cwd))
        self.lock.write_text(lockfile(("webpki-roots", argv[5])))
        return subprocess.CompletedProcess(argv, 0, "", "")

    def test_precise_smallest_fix_without_manifest_or_policy_changes(self):
        manifest = self.cwd / "Cargo.toml"
        policy = self.cwd / "deny.toml"
        manifest.write_text('[dependencies]\nwebpki-roots = "1"\n')
        policy.write_text('[advisories]\nignore = []\n')
        run = Mock(side_effect=self.successful_update)
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "fix_available")
        self.assertEqual(result["updates"], [{"package": "webpki-roots", "from": "1.0.9", "to": "1.0.10"}])
        self.assertEqual(run.call_count, 1)
        self.assertEqual(manifest.read_text(), '[dependencies]\nwebpki-roots = "1"\n')
        self.assertEqual(policy.read_text(), '[advisories]\nignore = []\n')
        self.assertEqual(self.report["updates"], [])

    def test_resolver_conflict_tries_next_lowest_precise_version(self):
        def run(argv, **kwargs):
            if argv[-1] == "1.0.10":
                self.lock.write_text("partial resolver write")
                return subprocess.CompletedProcess(argv, 101, "", "manifest requires >=1.0.11")
            self.assertEqual(self.lock.read_text(), self.original)
            return self.successful_update(argv, **kwargs)
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "fix_available")
        self.assertEqual(result["updates"][0]["to"], "1.0.11")

    def test_no_compatible_version_opens_blocker_and_preserves_lock(self):
        self.fetch.return_value = [index_version("webpki-roots", "2.0.0")]
        run = Mock()
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "blocked")
        self.assertIn("no non-yanked stable semver-compatible fix", result["errors"][0])
        self.assertEqual(self.lock.read_text(), self.original)
        run.assert_not_called()

    def test_no_published_patch_does_not_depend_on_registry_network(self):
        self.report["advisories"][0]["patched_versions"] = []
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=Mock())
        self.assertEqual(result["status"], "blocked")
        self.assertIn("no published safe upgrade", result["errors"][0])
        self.fetch.assert_not_called()

    def test_resolver_failure_preserves_lock_and_records_reason(self):
        def run(argv, **kwargs):
            self.lock.write_text("partial resolver write")
            return subprocess.CompletedProcess(argv, 101, "", "pinned by parent crate")
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "blocked")
        self.assertIn("pinned by parent crate", result["errors"][0])
        self.assertEqual(self.lock.read_text(), self.original)

    def test_timeout_rolls_back_partial_write(self):
        def run(argv, **kwargs):
            self.lock.write_text("partial resolver write")
            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "error")
        self.assertEqual(self.lock.read_text(), self.original)

    def test_second_blocker_rolls_back_successful_first_update(self):
        second = copy.deepcopy(self.report["advisories"][0])
        second.update(package="zzz-unmaintained", version="1.0.0", patched_versions=[])
        self.report["advisories"].append(second)
        self.original = lockfile(("webpki-roots", "1.0.9"), ("zzz-unmaintained", "1.0.0"))
        self.lock.write_text(self.original)
        def run(argv, **kwargs):
            self.lock.write_text(lockfile(("webpki-roots", argv[-1]), ("zzz-unmaintained", "1.0.0")))
            return subprocess.CompletedProcess(argv, 0, "", "")
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=run)
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["updates"], [])
        self.assertEqual(self.lock.read_text(), self.original)

    def test_transitive_replacement_is_recorded_without_redundant_update(self):
        # Resolve rustls-webpki first; its precise update also moves webpki-roots.
        self.report = scanned(RUSTLS, WEBPKI)
        self.original = lockfile(("webpki-roots", "1.0.9"), ("rustls-webpki", "0.103.14"))
        self.lock.write_text(self.original)
        fetch = Mock(return_value=[index_version("rustls-webpki", "0.103.16")])
        def run(argv, **kwargs):
            self.assertEqual(argv[3], "rustls-webpki@0.103.14")
            self.lock.write_text(lockfile(("webpki-roots", "1.0.10"), ("rustls-webpki", "0.103.16")))
            return subprocess.CompletedProcess(argv, 0, "", "")
        run = Mock(side_effect=run)
        result = SECURITY.remediate(self.report, self.cwd, fetch=fetch, run=run)
        self.assertEqual(result["status"], "fix_available")
        self.assertEqual(len(result["updates"]), 2)
        self.assertEqual(result["updates"][1], {"package": "webpki-roots", "from": "1.0.9", "to": "1.0.10"})
        self.assertEqual(run.call_count, 1)
        fetch.assert_called_once_with("rustls-webpki")

    def test_unsafe_transitive_replacement_is_blocked_and_rolled_back(self):
        self.report = scanned(RUSTLS, WEBPKI)
        self.original = lockfile(("webpki-roots", "1.0.9"), ("rustls-webpki", "0.103.14"))
        self.lock.write_text(self.original)
        fetch = Mock(return_value=[index_version("rustls-webpki", "0.103.16")])
        def run(argv, **kwargs):
            self.lock.write_text(lockfile(("webpki-roots", "2.0.0"), ("rustls-webpki", "0.103.16")))
            return subprocess.CompletedProcess(argv, 0, "", "")
        result = SECURITY.remediate(self.report, self.cwd, fetch=fetch, run=run)
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["updates"], [])
        self.assertEqual(self.lock.read_text(), self.original)

    def test_non_crates_io_source_is_left_for_human(self):
        self.report["advisories"][0]["source"] = "https://private.example/registry"
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=Mock())
        self.assertEqual(result["status"], "blocked")
        self.fetch.assert_not_called()

    def test_stale_report_cannot_update_different_lockfile(self):
        self.lock.write_text(lockfile(("webpki-roots", "1.0.10")))
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=Mock())
        self.assertEqual(result["status"], "blocked")
        self.fetch.assert_not_called()

    def test_incomplete_scan_cannot_be_remediated(self):
        result = SECURITY.remediate(SECURITY.empty_report("error"), self.cwd, fetch=self.fetch, run=Mock())
        self.assertEqual(result["status"], "error")
        self.fetch.assert_not_called()

    def test_cargo_success_without_selected_replacement_is_blocked(self):
        result = SECURITY.remediate(self.report, self.cwd, fetch=self.fetch, run=Mock(return_value=subprocess.CompletedProcess([], 0, "", "")))
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(self.lock.read_text(), self.original)


class CliTests(unittest.TestCase):
    def test_report_json_and_markdown_from_captured_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            raw, report, body = (directory / name for name in ("deny.jsonl", "report.json", "report.md"))
            raw.write_text(diagnostics(WEBPKI, errors=1))
            result = subprocess.run([sys.executable, str(SCRIPT), "scan", "--diagnostics", str(raw), "--exit-code", "1", "--output", str(report), "--markdown", str(body)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(report.read_text())["status"], "advisories")
            self.assertIn("RUSTSEC-2026-0381", body.read_text())
            self.assertIn("gongbu-api@0.2.4", body.read_text())

    def test_markdown_treats_scan_text_as_data(self):
        report = scanned(WEBPKI)
        report["advisories"][0]["title"] = "<script>bad</script>\n## headline"
        self.assertNotIn("<script>", SECURITY.markdown(report))
        self.assertIn("&lt;script&gt;", SECURITY.markdown(report))

    def test_classification_summary_and_tracking_link(self):
        baseline = scanned(WEBPKI)
        existing = SECURITY.parse_scan(diagnostics(WEBPKI, errors=1), 1, baseline)
        self.assertIn("All denied advisories already affect main", SECURITY.markdown(existing))
        mixed = SECURITY.parse_scan(diagnostics(WEBPKI, RUSTLS, errors=2), 1, baseline)
        self.assertIn("1 denied advisory/package/version combinations already affect main; 1 are introduced", SECURITY.markdown(mixed))
        introduced = SECURITY.parse_scan(diagnostics(WEBPKI, errors=1), 1, SECURITY.empty_report())
        self.assertIn("All denied advisory/package/version combinations are introduced", SECURITY.markdown(introduced))
        self.assertIn("/actions/workflows/security-advisories.yml", SECURITY.markdown(existing))


if __name__ == "__main__":
    unittest.main()
