#!/usr/bin/env python3
"""Offline checks for the automated system card: release resolution in the
proof-kit bootstrap, the renderer's publication guards, and the workflow's
privilege split. No network, build or provider access."""

import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
KITS = ROOT / "break-hubu"
sys.path.insert(0, str(KITS))
import bootstrap  # noqa: E402

COMMIT = "5344ab565171e1aa3d3ec628641cdbfce18372af"
OTHER = "86cc4b9764562d22c0b0d8e85e9909004f51546d"


def published(**overrides):
    release = {"tag_name": "v0.2.5", "draft": False, "prerelease": False,
               "body": f"Hubu product version: `v0.2.5`\n\nSource commit: `{COMMIT}`\n"}
    release.update(overrides)
    return release


class ReleaseResolution(unittest.TestCase):
    def test_stable_release_pins_its_published_source_commit(self):
        self.assertEqual(bootstrap.pinned_commit(published(), "v0.2.5"), COMMIT)

    def test_refuses_drafts_prereleases_and_other_tags(self):
        for overrides in ({"draft": True}, {"prerelease": True}, {"tag_name": "v0.2.4"}):
            with self.subTest(overrides=overrides), self.assertRaises(RuntimeError):
                bootstrap.pinned_commit(published(**overrides), "v0.2.5")

    def test_refuses_missing_or_ambiguous_source_commits(self):
        for body in ("No commit here", f"Source commit: `{COMMIT}`\nSource commit: `{OTHER}`", f"Source commit: {COMMIT}"):
            with self.subTest(body=body), self.assertRaises(RuntimeError):
                bootstrap.pinned_commit(published(body=body), "v0.2.5")

    def test_only_stable_tags_are_accepted(self):
        for release in ("v0.2.5-rc.1", "main-" + COMMIT, "0.2.5", "", None):
            with self.subTest(release=release), self.assertRaises(ValueError):
                bootstrap.build(release, cache=tempfile.mkdtemp())


def load_renderer():
    spec = importlib.util.spec_from_file_location("render_report", KITS / "render-report.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class RendererGuards(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "repo"
        self.evidence = Path(self.temporary.name) / "evidence"
        release = self.root / "site/app/releases/v0.2.5"
        release.mkdir(parents=True)
        (release / "page.tsx").write_text("export default function Page() { return null; }\n")
        # An independent unmeasured fixture: the repository's own releases may be measured.
        (release / "report.ts").write_text('import type { ReleaseReport } from "../report-types";\n' + load_renderer().PLACEHOLDER + "\n")
        (self.root / "break-hubu/cards").mkdir(parents=True)
        (self.root / "site/app/releases/claims.json").write_text((ROOT / "site/app/releases/claims.json").read_text())
        for episode in list(json.loads((ROOT / "site/app/releases/claims.json").read_text())["kits"])[:4]:
            (self.evidence / episode).mkdir(parents=True)
            self.write_run(episode)

    def tearDown(self):
        self.temporary.cleanup()

    def write_run(self, episode, **overrides):
        version = {"product_version": "v0.2.5", "source_commit": COMMIT, "executor_contract": "hubu-spend-executor-v4.5"}
        run = {"release": "v0.2.5", "source_commit": COMMIT, "kits_commit": OTHER, "kits_worktree_dirty": False,
               "binary_versions": {"hubu": version, "hubu-server": version}, "started_at": "2026-11-06T00:00:00+00:00",
               "platform": "macOS"}
        run.update(overrides)
        (self.evidence / episode / "run.json").write_text(json.dumps(run))

    def render(self):
        return subprocess.run([sys.executable, str(KITS / "render-report.py"), str(self.evidence), "--root", str(self.root)],
                              capture_output=True, text=True)

    def test_refuses_dirty_kits_checkouts(self):
        self.write_run("ep02-last-cent", kits_worktree_dirty=True)
        self.assertIn("clean committed kits checkout", self.render().stderr)

    def test_refuses_mixed_releases(self):
        self.write_run("ep03-crash-recovery", release="v0.2.4")
        self.assertIn("one released source", self.render().stderr)

    def test_requires_the_release_notes_page(self):
        (self.root / "site/app/releases/v0.2.5/page.tsx").unlink()
        self.assertIn("release notes page first", self.render().stderr)

    def test_never_replaces_a_measured_report(self):
        (self.root / "site/app/releases/v0.2.5/report.ts").write_text("export const report = { measured: true };\n")
        self.assertIn("already holds a measured report", self.render().stderr)

    def test_never_replaces_a_published_card(self):
        (self.root / "break-hubu/cards/v0.2.5.md").write_text("# published\n")
        self.assertIn("preserve prior publication", self.render().stderr)

    def test_refuses_binaries_from_another_source(self):
        version = {"product_version": "v0.2.5", "source_commit": OTHER, "executor_contract": "hubu-spend-executor-v4.5"}
        self.write_run("ep01-runaway", binary_versions={"hubu": version, "hubu-server": version})
        self.assertIn("must match the released source", self.render().stderr)


class SiteContract(unittest.TestCase):
    def test_release_reports_are_placeholders_or_rendered(self):
        renderer = load_renderer()
        reports = sorted((ROOT / "site/app/releases").glob("v*/report.ts"))
        self.assertTrue(reports)
        for report in reports:
            with self.subTest(release=report.parent.name):
                source = report.read_text()
                wrapper = report.parent / "report/page.tsx"
                if renderer.PLACEHOLDER in source:
                    self.assertFalse(wrapper.exists(), "an unmeasured release has no analysis page")
                else:
                    self.assertIn("Generated from captured public evidence by break-hubu/render-report.py.", source)
                    self.assertEqual(wrapper.read_text(), renderer.WRAPPER)


class ClaimsCatalog(unittest.TestCase):
    def setUp(self):
        self.catalog = json.loads((ROOT / "site/app/releases/claims.json").read_text())

    def test_lists_the_six_hubu_claims_in_order(self):
        self.assertEqual([claim["id"] for claim in self.catalog["claims"]], ["C1", "C2", "C3", "C4", "C5", "C6"])
        for claim in self.catalog["claims"]:
            self.assertTrue(claim["title"] and claim["invariant"], claim["id"])

    def test_every_challenge_belongs_to_a_claim_and_has_a_known_status(self):
        claims = {claim["id"] for claim in self.catalog["claims"]}
        ids = [challenge["id"] for challenge in self.catalog["challenges"]]
        self.assertEqual(len(ids), len(set(ids)))
        for challenge in self.catalog["challenges"]:
            with self.subTest(challenge=challenge["id"]):
                self.assertIn(challenge["claim"], claims)
                self.assertTrue(challenge["id"].startswith(challenge["claim"] + "."))
                self.assertIn(challenge["status"], {"kit", "planned", "blocked", "out-of-scope"})
                if challenge["status"] == "kit":
                    self.assertIn(challenge["kit"], self.catalog["kits"])
                else:
                    self.assertNotIn("kit", challenge)
                    self.assertTrue(challenge["reason"])

    def test_every_kit_measures_exactly_one_challenge_and_exists(self):
        measured = [challenge["kit"] for challenge in self.catalog["challenges"] if challenge.get("kit")]
        self.assertEqual(sorted(measured), sorted(self.catalog["kits"]))
        for kit, text in self.catalog["kits"].items():
            with self.subTest(kit=kit):
                for name in ("attack.py", "verify.py", "run.sh", "README.md"):
                    self.assertTrue((KITS / kit / name).is_file(), f"{kit}/{name}")
                for field in ("number", "title", "short", "question", "statement", "setup", "steps", "verify", "boundary"):
                    self.assertTrue(text[field], f"{kit}.{field}")

    def test_tier_rule(self):
        renderer = load_renderer()
        catalog = {"claims": [{"id": "C9"}], "challenges": [
            {"id": "C9.1", "claim": "C9", "status": "kit", "kit": "a"},
            {"id": "C9.2", "claim": "C9", "status": "kit", "kit": "b"},
            {"id": "C9.3", "claim": "C9", "status": "out-of-scope"}]}
        def tier(**verdicts):
            return renderer.claim_tier("C9", catalog, {"episodes": {k: {"verdict": v} for k, v in verdicts.items()}})
        self.assertEqual(tier(a="holds", b="holds-with-caveat"), dict(tier="verified", held=2, total=2, failed=0))
        self.assertEqual(tier(a="holds")["tier"], "partially-verified")
        self.assertEqual(tier()["tier"], "unverified")
        self.assertEqual(tier(a="holds", b="does-not-hold")["tier"], "does-not-hold")
        self.assertEqual(tier(a="not-tested", b="not-tested")["tier"], "unverified")

    def test_release_reports_are_placeholders_or_rendered(self):
        renderer = load_renderer()
        for report in sorted((ROOT / "site/app/releases").glob("v*/report.ts")):
            with self.subTest(release=report.parent.name):
                source = report.read_text()
                wrapper = report.parent / "report/page.tsx"
                if renderer.PLACEHOLDER in source:
                    self.assertFalse(wrapper.exists(), "an unmeasured release has no analysis page")
                else:
                    self.assertIn("Generated from captured public evidence by break-hubu/render-report.py.", source)
                    self.assertEqual(wrapper.read_text(), renderer.WRAPPER)
                    measured = renderer.load_report(report)["episodes"]
                    self.assertLessEqual(set(measured), set(self.catalog["kits"]))
                    bundle = json.loads((ROOT / "site/evidence" / report.parent.name / "bundle.json").read_text())
                    self.assertEqual(bundle["schema"], renderer.BUNDLE_SCHEMA)
                    self.assertTrue(all(any(name.startswith(kit + "/") for name in bundle["files"]) for kit in measured))


class EvidenceStore(unittest.TestCase):
    def test_archive_is_deterministic_and_matches_the_bundle(self):
        renderer = load_renderer()
        files = {"ep01-runaway/run.json": "{}\n", "summary.json": "{\"a\": 1}\n"}
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            renderer.write_store(Path(first), "v9.9.9", files)
            renderer.write_store(Path(second), "v9.9.9", dict(reversed(list(files.items()))))
            self.assertEqual((Path(first) / "evidence.zip").read_bytes(), (Path(second) / "evidence.zip").read_bytes())
            import zipfile
            with zipfile.ZipFile(Path(first) / "evidence.zip") as archive:
                self.assertEqual({name: archive.read(name).decode() for name in archive.namelist()}, files)
            self.assertEqual(renderer.read_bundle(Path(first) / "bundle.json"), files)

    def test_no_evidence_tree_is_committed_under_public(self):
        tracked = subprocess.run(["git", "ls-files", "site/public/break-hubu"], cwd=ROOT, capture_output=True, text=True).stdout
        self.assertEqual(tracked.strip(), "")


class WorkflowPrivileges(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/system-card.yml").read_text()
        _, jobs = self.workflow.split("\njobs:\n", 1)
        starts = list(re.finditer(r"(?m)^  ([a-z][a-z0-9_-]*):\s*$", jobs))
        self.jobs = {m.group(1): jobs[m.start():(starts[i + 1].start() if i + 1 < len(starts) else len(jobs))]
                     for i, m in enumerate(starts)}

    def test_default_permissions_are_read_only(self):
        self.assertRegex(self.workflow, r"(?m)^permissions:\n  contents: read$")

    def test_only_the_publish_job_can_write(self):
        self.assertEqual(set(self.jobs), {"resolve", "measure", "publish"})
        for name, job in self.jobs.items():
            writes = re.findall(r"(?m)^\s+[a-z-]+: write$", job)
            self.assertEqual(bool(writes), name == "publish", name)

    def test_release_code_runs_without_persisted_credentials(self):
        self.assertIn("persist-credentials: false", self.jobs["measure"])
        self.assertNotIn("run-all.sh", self.jobs["publish"])

    def test_never_merges_or_touches_providers(self):
        for forbidden in ("gh pr merge", "--auto", "gh release edit", "gh release upload", "GONGBU_LIVE_", "I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND"):
            self.assertNotIn(forbidden, self.workflow)

    def test_actions_are_pinned_by_commit(self):
        for action in re.findall(r"uses: (\S+)", self.workflow):
            self.assertRegex(action, r"^[a-z-]+/[a-z-]+@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()
