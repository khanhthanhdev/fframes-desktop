#!/usr/bin/env python3
"""Offline tests of the consumer release gate and the package verifier."""
import copy
import hashlib
import importlib.util
import json
import re
import tempfile
import unittest
import zipfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate = load("release-gate")
verify = load("verify-package")

TAG = "a" * 40
LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"


def policy(enabled=True, eligible=(LINUX,)):
    return {
        "publication": {
            "consumer_release_enabled": enabled,
            "trusted_release_keys_configured": enabled,
        },
        "eligible_targets": list(eligible),
        "targets": [{"triple": LINUX}, {"triple": WINDOWS}, {"triple": "aarch64-apple-darwin"}],
    }


def job(name, full=True):
    return {
        "name": name,
        "conclusion": "success",
        "steps": [
            {"name": "Test desktop workspace", "conclusion": "success"},
            {"name": gate.FULL_ONLY_STEP, "conclusion": "success" if full else "skipped"},
        ],
    }


def self_hosted(full=True, sha=TAG, conclusion="success"):
    return {
        "workflowName": gate.SELF_HOSTED_WORKFLOW,
        "headSha": sha,
        "conclusion": conclusion,
        "jobs": [job("Desktop validation (self-hosted Linux)", full), job("Desktop validation (self-hosted Windows)", full)],
    }


def desktop(sha=TAG, branch="main", conclusion="success"):
    return {"workflowName": gate.DESKTOP_WORKFLOW, "headSha": sha, "headBranch": branch, "conclusion": conclusion}


class ReleaseGateTests(unittest.TestCase):
    def check(self, release_policy=None, self_hosted_runs=None, desktop_runs=None):
        return gate.check(
            release_policy or policy(),
            TAG,
            [self_hosted()] if self_hosted_runs is None else self_hosted_runs,
            [desktop()] if desktop_runs is None else desktop_runs,
        )

    def test_a_tag_with_a_full_run_and_a_main_run_publishes_the_eligible_targets(self):
        self.assertEqual(self.check(), [LINUX])

    def test_a_disabled_consumer_release_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "disabled"):
            self.check(policy(enabled=False))

    def test_no_eligible_target_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "No target"):
            self.check(policy(eligible=()))

    def test_an_eligible_target_the_policy_does_not_build_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "riscv64"):
            self.check(policy(eligible=(LINUX, "riscv64gc-unknown-linux-gnu")))

    def test_a_tag_with_only_a_fast_run_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "full"):
            self.check(self_hosted_runs=[self_hosted(full=False)])

    def test_a_full_run_with_one_job_fast_is_refused(self):
        run = self_hosted()
        run["jobs"][1] = job("Desktop validation (self-hosted Windows)", full=False)
        with self.assertRaisesRegex(gate.Refused, "full"):
            self.check(self_hosted_runs=[run])

    def test_a_failed_full_run_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "full"):
            self.check(self_hosted_runs=[self_hosted(conclusion="failure")])

    def test_a_full_run_of_another_commit_is_refused(self):
        with self.assertRaisesRegex(gate.Refused, "full"):
            self.check(self_hosted_runs=[self_hosted(sha="b" * 40)])

    def test_a_run_of_another_workflow_does_not_count_as_full(self):
        run = self_hosted()
        run["workflowName"] = "Something else"
        with self.assertRaisesRegex(gate.Refused, "full"):
            self.check(self_hosted_runs=[run])

    def test_a_desktop_run_off_main_or_of_another_commit_is_refused(self):
        for run in [desktop(branch="feature"), desktop(sha="b" * 40), desktop(conclusion="failure")]:
            with self.subTest(run=run), self.assertRaisesRegex(gate.Refused, "main"):
                self.check(desktop_runs=[run])

    def test_one_passing_run_among_failed_ones_is_enough(self):
        runs = [self_hosted(full=False), self_hosted()]
        self.assertEqual(self.check(self_hosted_runs=runs, desktop_runs=[desktop(branch="x"), desktop()]), [LINUX])

    def test_the_main_entry_prints_the_targets_and_exits_nonzero_on_refusal(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            (temp / "policy.json").write_text(json.dumps(policy()))
            (temp / "self-hosted.json").write_text(json.dumps([self_hosted()]))
            (temp / "desktop.json").write_text(json.dumps([desktop()]))
            args = ["--policy", str(temp / "policy.json"), "--tag-commit", TAG,
                    "--self-hosted-runs", str(temp / "self-hosted.json"), "--desktop-runs", str(temp / "desktop.json")]
            self.assertEqual(gate.main(args), LINUX)
            (temp / "self-hosted.json").write_text(json.dumps([self_hosted(full=False)]))
            with self.assertRaises(SystemExit) as refused:
                gate.main(args)
            self.assertNotEqual(refused.exception.code, 0)

    def test_the_committed_policy_keeps_the_consumer_release_disabled(self):
        committed = json.loads((SCRIPTS.parent / "packaging/release-policy.json").read_text())
        self.assertFalse(committed["publication"]["consumer_release_enabled"])
        self.assertIsInstance(committed["eligible_targets"], list)
        with self.assertRaisesRegex(gate.Refused, "disabled"):
            self.check(committed)


def write_package(directory: Path, target: str, files: dict[str, bytes], inventory_target=None, extra=None):
    root = f"fframes-studio-{target}"
    inventory = {
        "target": inventory_target or target,
        "sdk_included": True,
        "files": [
            {"path": path, "size_bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            for path, data in sorted(files.items())
        ],
    }
    archive = directory / f"{root}.zip"
    with zipfile.ZipFile(archive, "w") as package:
        for path, data in {**files, **(extra or {})}.items():
            package.writestr(f"{root}/{path}", data)
        package.writestr(f"{root}/inventory.json", json.dumps(inventory))
    return archive


FILES = {"fframes-studio": b"binary", "qualification.json": b"{}", "sdk/manifest.json": b"[]"}


class VerifyPackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.dir = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def copy_path(self, archive):
        """Where a changed copy of `archive` goes: same name (the name is the root), other folder."""
        folder = self.dir / "changed"
        folder.mkdir()
        return folder / archive.name

    def test_the_root_folder_is_named_like_the_archive(self):
        archive = write_package(self.dir, LINUX, FILES)
        renamed = archive.rename(self.dir / "renamed.zip")
        with self.assertRaisesRegex(verify.Refused, "outside renamed/"):
            verify.verify(policy(), LINUX, renamed)

    def test_a_package_matching_its_inventory_verifies(self):
        verify.verify(policy(), LINUX, write_package(self.dir, LINUX, FILES))

    def test_an_ineligible_target_is_refused(self):
        with self.assertRaisesRegex(verify.Refused, "not eligible"):
            verify.verify(policy(), WINDOWS, write_package(self.dir, WINDOWS, FILES))

    def test_a_package_of_another_target_is_refused(self):
        with self.assertRaisesRegex(verify.Refused, "target"):
            verify.verify(policy(), LINUX, write_package(self.dir, LINUX, FILES, inventory_target=WINDOWS))

    def test_a_changed_file_is_refused(self):
        archive = write_package(self.dir, LINUX, FILES)
        tampered = self.copy_path(archive)
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(tampered, "w") as target:
            for item in source.infolist():
                data = source.read(item)
                target.writestr(item, b"other" if item.filename.endswith("/fframes-studio") else data)
        with self.assertRaisesRegex(verify.Refused, "fframes-studio does not match"):
            verify.verify(policy(), LINUX, tampered)

    def test_a_file_missing_from_the_inventory_is_refused(self):
        archive = write_package(self.dir, LINUX, FILES, extra={"unlisted.dll": b"x"})
        with self.assertRaisesRegex(verify.Refused, "unlisted.dll is in the package but not in its inventory"):
            verify.verify(policy(), LINUX, archive)

    def test_a_listed_file_missing_from_the_package_is_refused(self):
        archive = write_package(self.dir, LINUX, FILES)
        trimmed = self.copy_path(archive)
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(trimmed, "w") as target:
            for item in source.infolist():
                if not item.filename.endswith("sdk/manifest.json"):
                    target.writestr(item, source.read(item))
        with self.assertRaisesRegex(verify.Refused, "manifest.json is in the inventory but not in the package"):
            verify.verify(policy(), LINUX, trimmed)

    def test_a_path_outside_the_package_root_is_refused(self):
        archive = write_package(self.dir, LINUX, FILES)
        with zipfile.ZipFile(archive, "a") as package:
            package.writestr("elsewhere/file", b"x")
        with self.assertRaisesRegex(verify.Refused, "outside"):
            verify.verify(policy(), LINUX, archive)


class ReleaseWorkflowTests(unittest.TestCase):
    def test_the_gate_names_the_workflows_and_the_full_only_step_as_they_are(self):
        workflows = SCRIPTS.parents[1] / ".github/workflows"
        desktop_ci = (workflows / "desktop.yml").read_text()
        self_hosted = (workflows / "desktop-self-hosted.yml").read_text()
        self.assertTrue(desktop_ci.startswith(f"name: {gate.DESKTOP_WORKFLOW}\n"))
        self.assertTrue(self_hosted.startswith(f"name: {gate.SELF_HOSTED_WORKFLOW}\n"))
        jobs = re.split(r"\n  (?=[a-z][\w-]*:\n)", "\n" + self_hosted.split("\njobs:\n", 1)[1])[1:]
        self.assertEqual(len(jobs), 2)
        for job in jobs:
            step = job.split(f"- name: {gate.FULL_ONLY_STEP}\n", 1)
            self.assertEqual(len(step), 2, job.split("\n", 1)[0])
            self.assertEqual(step[1].split("\n", 1)[0].strip(), "if: inputs.full")

    def test_the_consumer_release_runs_the_gate_and_the_verifier_and_a_dry_run_publishes_nothing(self):
        workflow = (SCRIPTS.parents[1] / ".github/workflows/desktop-release.yml").read_text()
        consumer = workflow.split("\n  publish:\n", 1)[1]
        self.assertIn("release-gate.py", consumer)
        self.assertIn("verify-package.py", consumer)
        self.assertNotIn("desktop/qualification", workflow)
        self.assertNotIn("test-qualification", workflow)
        self.assertNotIn("validate-qualification", workflow)
        publish = consumer.split("- name: Publish GitHub Release", 1)[1]
        self.assertIn("inputs.dry_run != true", publish.split("run:", 1)[0])
        preview = workflow.split("\n  publish-preview:\n", 1)[1].split("\n  publish:\n", 1)[0]
        self.assertNotIn("inputs.dry_run", preview.split("steps:", 1)[0])
        preview_publish = preview.split("- name: Publish GitHub release", 1)[1]
        self.assertIn("inputs.dry_run != true", preview_publish.split("run:", 1)[0])
        self.assertIn("studio-preview-dry-run", preview)
        self.assertIn("Preview dry run summary", preview)


if __name__ == "__main__":
    unittest.main()
