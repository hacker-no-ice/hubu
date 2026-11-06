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
        (release / "report.ts").write_text((ROOT / "site/app/releases/v0.2.5/report.ts").read_text())
        (self.root / "break-hubu/cards").mkdir(parents=True)
        for episode in load_renderer().EPISODES:
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
    def test_unmeasured_report_is_the_renderers_placeholder(self):
        placeholder = load_renderer().PLACEHOLDER
        self.assertIn(placeholder, (ROOT / "site/app/releases/v0.2.5/report.ts").read_text())

    def test_measured_release_wrapper_matches_the_renderer(self):
        self.assertEqual((ROOT / "site/app/releases/v0.2.4/report/page.tsx").read_text(), load_renderer().WRAPPER)


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
