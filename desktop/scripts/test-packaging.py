#!/usr/bin/env python3
"""Regression checks for artifact integrity and qualification claims."""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
import signal
import zipfile


def signal_kill():
    # Fake process-table tests compare the requested signal without delivering it.
    return getattr(signal, "SIGKILL", signal.SIGTERM)


EXE_SUFFIX = ".exe" if os.name == "nt" else ""


def load(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


assembly = load("assemble-phase-zero-sdk")
packaging = load("package-phase-zero")
finalizing = load("finalize-native-package")
debian_packaging = load("package-linux-deb")
qualification = load("validate-qualification")
harness = load("qualify-m3-agent")


class ArtifactTests(unittest.TestCase):
    def m2_record(self):
        record = json.loads((assembly.ROOT / "desktop/qualification/m2-results.json").read_text())
        for gate in record["gates"].values():
            gate["status"] = "NOT_RUN"
            gate.pop("evidence", None)
        record["metrics"] = {}
        record["target_platform"]["status"] = "PENDING"
        record["environment"].update(clock_source="TEST_ONLY", physical_audio_evidence=False, native_platform_evidence=False)
        return record

    def validate_temp_record(self, root, record):
        path = Path(root) / "m2-results.json"
        path.write_text(json.dumps(record))
        return qualification.validate(path)

    def test_legacy_sdk_manifest_has_stable_digest_and_no_preview_fields(self):
        raw = json.loads((assembly.ROOT / "desktop/packaging/sdk/phase-zero-sdk.json").read_text())
        self.assertNotIn("preview_contract_versions", raw)
        canonical = json.dumps(raw, separators=(",", ":"))
        self.assertEqual(__import__("hashlib").sha256(canonical.encode()).hexdigest(), "105670909607f0b6043ddaee5e2b98a9f2b2614271831a5cbff804b3983e764b")

    @unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("dpkg-deb") and shutil.which("dpkg-shlibdeps"), "requires Linux Debian packaging tools")
    def test_linux_deb_has_installed_shell_layout_and_derived_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "phase-zero"
            (source / "bin").mkdir(parents=True)
            (source / "notices").mkdir()
            binary = Path(shutil.which("true")).resolve()
            for name in debian_packaging.BINARIES:
                shutil.copy2(binary, source / "bin" / name)
            (source / "notices/LICENSE.txt").write_text("test notice")
            output = root / "fframes-studio.deb"

            package = debian_packaging.package(
                source,
                output,
                "fframes Studio Tests <test@example.invalid>",
            )
            package_bytes = package.read_bytes()
            with self.assertRaisesRegex(ValueError, "output already exists"):
                debian_packaging.package(
                    source,
                    output,
                    "fframes Studio Tests <test@example.invalid>",
                )
            self.assertEqual(package.read_bytes(), package_bytes, "an existing package must not be replaced")
            fields = subprocess.run(
                ["dpkg-deb", "--field", str(package)],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            self.assertIn("Package: fframes-studio", fields)
            self.assertIn("Architecture: amd64", fields)
            self.assertIn("Depends: libc6", fields)
            installed = root / "installed"
            subprocess.run(["dpkg-deb", "--extract", str(package), str(installed)], check=True)
            self.assertTrue((installed / "usr/bin/fframes-studio").stat().st_mode & 0o111)
            self.assertIn("/opt/fframes-studio/bin/fframes-studio studio", (installed / "usr/bin/fframes-studio").read_text())
            self.assertTrue((installed / "usr/share/applications/fframes-studio.desktop").is_file())
            self.assertTrue((installed / "opt/fframes-studio/notices/LICENSE.txt").is_file())
            self.assertFalse((installed / "opt/fframes-studio/sdk").exists(), "the app package must not embed the separate SDK")

    def test_bundled_ffmpeg_links_supplied_install_without_download_feature(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "fframes-media").mkdir()
            manifest = root / "fframes-media/Cargo.toml"
            manifest.write_text((assembly.ROOT / "fframes-media/Cargo.toml").read_text())
            assembly.configure_bundled_ffmpeg(root)
            dependencies = tomllib.loads(manifest.read_text())["target"]
            native = dependencies['cfg(not(any(target_arch = "wasm32", windows, fframes_docsrs)))']["dependencies"]
            self.assertEqual(native["ffmpeg-sys-fframes"]["features"], ["static"])

    def test_archive_metadata_matches_bytes_and_corruption_rejects(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            source.mkdir()
            (source / "file").write_text("actual archive bytes")
            bundle = root / "bundle"
            (bundle / "artifacts").mkdir(parents=True)
            artifacts = [assembly.archive(source, bundle, name, name) for name in ["toolchain", "ffmpeg", "framework"]]
            (bundle / "compatibility.json").write_text(json.dumps({"artifacts": artifacts}))
            packaging.verify_sdk(bundle)
            (bundle / artifacts[0]["url"].removeprefix("file://")).write_bytes(b"corrupted")
            with self.assertRaises(ValueError):
                packaging.verify_sdk(bundle)

    def test_host_ffmpeg_cannot_be_mislabeled_as_version_nine(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "include/libavutil").mkdir(parents=True)
            (root / "include/libavutil/ffversion.h").write_text('#define FFMPEG_VERSION "6.1.1"')
            with self.assertRaisesRegex(ValueError, "found 6.1.1"):
                assembly.validate_ffmpeg(root, "9.0.0")
            (root / "include/libavutil/ffversion.h").write_text('#define FFMPEG_VERSION "n9.0.2-22-g46d8f462ee-20261001"')
            for lib in ["libavcodec", "libavformat", "libavutil", "libswscale", "libswresample"]:
                (root / "include" / lib).mkdir(parents=True, exist_ok=True)
                (root / "lib").mkdir(parents=True, exist_ok=True)
                (root / "lib" / f"{lib}.lib").write_text("stub")
            self.assertEqual(assembly.validate_ffmpeg(root, "9.0.0"), "n9.0.2-22-g46d8f462ee-20261001")

    def test_standalone_workspace_serializer_preserves_nested_lints(self):
        workspace = {"members": ["fframes"], "lints": {"rust": {"unexpected_cfgs": {"level": "warn", "check-cfg": ["cfg(example)"]}}}}
        document = "[workspace]\n" + "\n".join(f"{key} = {assembly.toml_value(value)}" for key, value in workspace.items())
        self.assertEqual(tomllib.loads(document)["workspace"], workspace)

    def test_qualification_pass_requires_real_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "results.json"
            record = json.loads((assembly.ROOT / "desktop/qualification/m0-results.json").read_text())
            # The repository ledger can acquire real evidence; this fixture
            # starts unqualified independently of its current native results.
            for name, gate in record["gates"].items():
                if name == "acp_task":
                    gate["status"] = "NOT_RUN"
                else:
                    gate["passed"] = False
                gate.pop("evidence", None)
            record["target_platform"]["status"] = "PENDING"
            path.write_text(json.dumps(record))
            qualification.validate(path)
            record["gates"]["gpui_startup"]["passed"] = True
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "without evidence"):
                qualification.validate(path)

    def test_legacy_m0_record_without_discriminator_remains_valid(self):
        record = qualification.validate(assembly.ROOT / "desktop/qualification/m0-results.json")
        self.assertNotIn("kind", record)

    def test_unknown_explicit_qualification_kind_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            record = self.m2_record()
            record["kind"] = "m02"
            with self.assertRaisesRegex(ValueError, "Unsupported qualification kind"):
                self.validate_temp_record(temp, record)

    def test_m2_requires_exact_gates_criteria_and_pass_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            record = self.m2_record()
            del record["gates"]["cleanup"]
            with self.assertRaisesRegex(ValueError, "Invalid M2 gates"):
                self.validate_temp_record(temp, record)
            record = self.m2_record()
            record["gates"]["cleanup"]["criteria"] = ""
            with self.assertRaisesRegex(ValueError, "Missing qualification criteria"):
                self.validate_temp_record(temp, record)
            record = self.m2_record()
            record["gates"]["cleanup"]["status"] = "PASSED"
            with self.assertRaisesRegex(ValueError, "without evidence"):
                self.validate_temp_record(temp, record)

    def test_m2_rejects_missing_and_changed_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record = self.m2_record()
            gate = record["gates"]["cleanup"]
            gate["status"] = "PASSED"
            gate["evidence"] = [{"path": "proof.txt", "sha256": "0" * 64}]
            with self.assertRaisesRegex(ValueError, "Missing qualification evidence"):
                self.validate_temp_record(root, record)
            (root / "proof.txt").write_text("changed")
            with self.assertRaisesRegex(ValueError, "Evidence changed"):
                self.validate_temp_record(root, record)

    def test_m2_rejects_boolean_string_nan_and_infinite_metrics(self):
        invalid = [True, "1", float("nan"), float("inf")]
        with tempfile.TemporaryDirectory() as temp:
            for value in invalid:
                record = self.m2_record()
                record["metrics"]["seek_p95_ms"] = {"value": value, "unit": "ms"}
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Invalid"):
                    self.validate_temp_record(temp, record)

    def test_m2_rejects_unmet_timing_and_resource_measurements(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured")
            digest = __import__("hashlib").sha256(proof.read_bytes()).hexdigest()
            record = self.m2_record()
            record["gates"]["resource_stress"].update(
                status="PASSED", evidence=[{"path": proof.name, "sha256": digest}]
            )
            record["metrics"] = {
                "session_duration_minutes": {"value": 9, "unit": "min"},
                "seek_count": {"value": 1999, "unit": "count"},
                "rebuild_count": {"value": 49, "unit": "count"},
                "seek_p95_ms": {"value": 151, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2.1, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 36, "unit": "ms"},
            }
            with self.assertRaisesRegex(ValueError, "stress counts"):
                self.validate_temp_record(root, record)

    def test_m2_test_clock_cannot_pass_output_clock(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured")
            evidence = [{"path": proof.name, "sha256": __import__("hashlib").sha256(proof.read_bytes()).hexdigest()}]
            record = self.m2_record()
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 150, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 35.34, "unit": "ms"},
            }
            record["gates"]["output_clock"].update(status="PASSED", evidence=evidence)
            with self.assertRaisesRegex(ValueError, "physical live-output"):
                self.validate_temp_record(root, record)

    def test_virtual_resource_pass_does_not_invent_physical_timing_or_qualify_platform(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            proof = root / "proof.txt"
            proof.write_text("measured resources, not DAC timing")
            evidence = [{"path": proof.name, "sha256": __import__("hashlib").sha256(proof.read_bytes()).hexdigest()}]
            record = self.m2_record()
            record["gates"]["resource_stress"].update(status="PASSED", evidence=evidence)
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 149, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": -0.5, "unit": "MiB/min"},
            }
            self.validate_temp_record(root, record)
            record["gates"]["output_clock"].update(status="PASSED", evidence=record["gates"]["resource_stress"]["evidence"])
            with self.assertRaisesRegex(ValueError, "Missing required M2 measurements"):
                self.validate_temp_record(root, record)
            record = self.m2_record()
            for gate in record["gates"].values():
                gate.update(status="PASSED", evidence=evidence)
            record["target_platform"]["status"] = "QUALIFIED"
            record["metrics"] = {
                "session_duration_minutes": {"value": 10, "unit": "min"},
                "seek_count": {"value": 2000, "unit": "count"},
                "rebuild_count": {"value": 50, "unit": "count"},
                "seek_p95_ms": {"value": 150, "unit": "ms"},
                "rss_slope_mib_per_min": {"value": 2, "unit": "MiB/min"},
                "video_frame_duration_ms": {"value": 33.34, "unit": "ms"},
                "timestamp_residual_ms": {"value": 2, "unit": "ms"},
                "av_error_ms": {"value": 35.34, "unit": "ms"},
            }
            with self.assertRaisesRegex(ValueError, "physical live-output"):
                self.validate_temp_record(root, record)

    def test_finalizer_rehashes_signed_files_and_replaces_the_archive(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            package = root / "fframes-studio-x86_64-pc-windows-msvc"
            binary = package / "bin/fframes-studio.exe"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"unsigned executable")
            qualification_record = {
                "target_platform": {"triple": "x86_64-pc-windows-msvc"},
                "build_evidence": {"sdk_included": True},
            }
            (package / "qualification.json").write_text(json.dumps(qualification_record))
            packaging.write_inventory(package, "x86_64-pc-windows-msvc", True)
            archive = Path(shutil.make_archive(str(package), "zip", root, package.name))
            original_archive = archive.read_bytes()

            binary.write_bytes(b"authenticode-signed executable")
            finalizing.finalize(package)

            inventory = json.loads((package / "inventory.json").read_text())
            file_records = {item["path"]: item for item in inventory["files"]}
            expected_digest = hashlib.sha256(binary.read_bytes()).hexdigest()
            self.assertEqual(file_records["bin/fframes-studio.exe"]["sha256"], expected_digest)
            self.assertNotIn("inventory.json", file_records)
            self.assertNotEqual(archive.read_bytes(), original_archive)
            with zipfile.ZipFile(archive) as bundle:
                self.assertEqual(
                    bundle.read(f"{package.name}/bin/fframes-studio.exe"),
                    b"authenticode-signed executable",
                )
                archived_inventory = json.loads(bundle.read(f"{package.name}/inventory.json"))
                self.assertEqual(archived_inventory["files"], inventory["files"])
            self.assertEqual(
                sorted(path.name for path in root.glob(f".{package.name}.*.zip")),
                [],
            )

    @unittest.skipIf(os.name == "nt", "symlink creation may require elevated Windows privileges")
    def test_finalizer_refuses_symlinks_inside_package(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            package = root / "package"
            package.mkdir()
            (package / "qualification.json").write_text(
                json.dumps(
                    {
                        "target_platform": {"triple": "x86_64-unknown-linux-gnu"},
                        "build_evidence": {"sdk_included": False},
                    }
                )
            )
            outside = root / "outside"
            outside.write_text("outside package")
            (package / "linked-file").symlink_to(outside)

            with self.assertRaisesRegex(ValueError, "symlink"):
                finalizing.finalize(package)

    def test_release_workflow_requires_native_signing_for_tag_or_opt_in_builds(self):
        workflow = (assembly.ROOT / ".github/workflows/desktop-release.yml").read_text()
        mac_signing = (assembly.ROOT / "desktop/scripts/sign-macos-package.sh").read_text()
        windows_signing = (assembly.ROOT / "desktop/scripts/sign-windows-package.ps1").read_text()

        for secret in (
            "APPLE_DEVELOPER_ID_P12_BASE64",
            "APPLE_DEVELOPER_ID_P12_PASSWORD",
            "APPLE_DEVELOPER_IDENTITY",
            "APPLE_NOTARY_KEY_P8_BASE64",
            "APPLE_NOTARY_KEY_ID",
            "APPLE_NOTARY_ISSUER_ID",
        ):
            self.assertIn(secret, workflow)
            self.assertIn(secret, mac_signing)
        for secret in ("WINDOWS_CODESIGN_PFX_BASE64", "WINDOWS_CODESIGN_PFX_PASSWORD"):
            self.assertIn(secret, workflow)
            self.assertIn(secret, windows_signing)

        self.assertIn("inputs.sign_artifacts == true", workflow)
        self.assertIn("github.event_name == 'push'", workflow)
        self.assertIn("codesign --verify --deep --strict", mac_signing)
        self.assertIn("notarytool submit", mac_signing)
        self.assertIn("stapler staple", mac_signing)
        self.assertIn("verify /pa /all /v", windows_signing)
        self.assertIn("attestations: write", workflow)
        self.assertIn("id-token: write", workflow)
        self.assertIn("actions/attest-build-provenance@v3", workflow)
        self.assertLess(
            workflow.index("Sign and notarize macOS app package"),
            workflow.index("Attest native app package provenance"),
        )
        self.assertLess(
            workflow.index("Sign Windows executables and FFmpeg DLLs"),
            workflow.index("Attest native app package provenance"),
        )
        self.assertLess(
            workflow.index("Check macOS signing credentials before building"),
            workflow.index("Focused tests and lint"),
        )
        self.assertLess(
            workflow.index("Check Windows signing credentials before building"),
            workflow.index("Focused tests and lint"),
        )
        mac_preflight = workflow.split("Check macOS signing credentials before building", 1)[1].split(
            "Check Windows signing credentials before building", 1
        )[0]
        windows_preflight = workflow.split("Check Windows signing credentials before building", 1)[1].split(
            "Set native build directory", 1
        )[0]
        self.assertIn("APPLE_DEVELOPER_ID_P12_BASE64", mac_preflight)
        self.assertNotIn("WINDOWS_CODESIGN_PFX_BASE64", mac_preflight)
        self.assertIn("WINDOWS_CODESIGN_PFX_BASE64", windows_preflight)
        self.assertNotIn("APPLE_DEVELOPER_ID_P12_BASE64", windows_preflight)
        mac_packager = (assembly.ROOT / "desktop/scripts/package-phase-zero.py").read_text()
        self.assertIn('"CFBundleInfoDictionaryVersion": "6.0"', mac_packager)
        self.assertIn('"CFBundleVersion": "0.1.0"', mac_packager)


def extract_preserving_modes(archive, destination):
    """What `unzip` does: extract and restore the Unix modes stored in the archive."""
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            target = bundle.extract(info, destination)
            mode = info.external_attr >> 16
            if mode:
                os.chmod(target, mode)


class InstalledHelperTests(unittest.TestCase):
    """The agent runs `studio-tools --capability <file> <tool>` and MCP uses `studio-mcp`; the app
    finds both next to its own executable. These tests use the INSTALLED layout (an extracted
    archive), never repository paths."""

    def installed_package(self, root, helpers=packaging.PACKAGED_BINARIES):
        release = root / "release"
        release.mkdir()
        suffix = EXE_SUFFIX
        for name in packaging.PACKAGED_BINARIES:
            program = release / f"{name}{suffix}"
            program.write_text(f"#!{sys.executable}\nprint({name!r})\n")
            program.chmod(0o755)
        output = root / "pkg"
        (output / "bin").mkdir(parents=True)
        packaging.install_binaries(release, output / "bin", helpers, suffix)
        target = "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"
        packaging.write_inventory(output, target, False)
        archive = shutil.make_archive(str(output), "zip", root, "pkg")
        installed = root / "installed"
        extract_preserving_modes(archive, installed)
        return installed / "pkg"

    def test_installed_package_has_both_helpers_next_to_the_application(self):
        with tempfile.TemporaryDirectory() as temp:
            package = self.installed_package(Path(temp))
            suffix = EXE_SUFFIX
            app = package / "bin" / f"fframes-studio{suffix}"
            packaging.verify_helpers(app, suffix)
            for name in packaging.HELPER_BINARIES:
                sibling = packaging.sibling_binary(app, name, suffix)
                self.assertEqual(sibling, package / "bin" / f"{name}{suffix}")
                self.assertTrue(sibling.is_file(), name)
                if os.name != "nt":
                    self.assertTrue(os.access(sibling, os.X_OK), name)
                # Runs from the installed location with no repository on its path.
                command = [sys.executable, str(sibling)] if os.name == "nt" else [str(sibling)]
                path = os.environ.get("PATH", "") if os.name == "nt" else "/usr/bin:/bin"
                run = subprocess.run(command, cwd=temp, env={"PATH": path}, capture_output=True, text=True)
                self.assertEqual(run.stdout.strip(), name)
            inventory = {item["path"]: item for item in json.loads((package / "inventory.json").read_text())["files"]}
            for name in packaging.PACKAGED_BINARIES:
                relative = f"bin/{name}{suffix}"
                self.assertIn(relative, inventory)
                digest = hashlib.sha256((package / relative).read_bytes()).hexdigest()
                self.assertEqual(inventory[relative]["sha256"], digest)

    def test_a_package_without_a_helper_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            package = self.installed_package(Path(temp), helpers=("fframes-studio", "studio_setup", "studio-tools"))
            with self.assertRaisesRegex(ValueError, "studio-mcp"):
                packaging.verify_helpers(package / "bin" / f"fframes-studio{EXE_SUFFIX}", EXE_SUFFIX)

    def test_python_lookup_mirrors_the_rust_sibling_lookup(self):
        source = (assembly.ROOT / "desktop/app/src/agent_tools.rs").read_text()
        body = source[source.index("pub fn sibling_binary") :].split("\n}\n", 1)[0]
        for needle in ("current_exe()", ".parent()?", "EXE_SUFFIX", "is_file()"):
            self.assertIn(needle, body)
        self.assertEqual(sorted(packaging.HELPER_BINARIES), ["studio-mcp", "studio-tools"])
        manifest = tomllib.loads((assembly.ROOT / "desktop/app/Cargo.toml").read_text())
        self.assertEqual({b["name"] for b in manifest["bin"]}, {"fframes-studio", *packaging.HELPER_BINARIES})

    def test_built_helpers_start_from_an_installed_directory(self):
        debug = assembly.ROOT / "desktop/target/debug"
        suffix = EXE_SUFFIX
        built = [debug / f"{name}{suffix}" for name in packaging.HELPER_BINARIES]
        if not all(path.is_file() for path in built):
            self.skipTest("run `cargo build --locked -p fframes-studio` first: the helpers are not built")
        with tempfile.TemporaryDirectory() as temp:
            installed = Path(temp) / "installed/bin"
            installed.mkdir(parents=True)
            for path in built:
                shutil.copy2(path, installed)
            app = installed / f"fframes-studio{suffix}"
            app.write_text("#!/bin/sh\n")
            app.chmod(0o755)
            packaging.verify_helpers(app, suffix)
            env = {"PATH": os.environ.get("PATH", "") if os.name == "nt" else "/usr/bin:/bin"}
            tools = subprocess.run([str(installed / f"studio-tools{suffix}"), "--help"], cwd=temp, env=env, capture_output=True, text=True)
            self.assertEqual(tools.returncode, 0, tools.stderr)
            self.assertIn("--capability", tools.stdout)
            mcp = subprocess.run([str(installed / f"studio-mcp{suffix}"), "--protocol"], cwd=temp, env=env, capture_output=True, text=True)
            self.assertEqual(mcp.returncode, 0, mcp.stderr)
            self.assertIn("2025-06-18", mcp.stdout)


class M3LedgerTests(unittest.TestCase):
    LEDGER = assembly.ROOT / "desktop/qualification/m3-results.json"

    def copied(self, root):
        """The shipped ledger and its evidence directory, copied so tests can mutate them."""
        record = json.loads(self.LEDGER.read_text())
        evidence = {Path(item["path"]).parts[1] for gate in record["gates"].values() for item in gate["evidence"]}
        (name,) = evidence
        shutil.copytree(self.LEDGER.parent / "evidence" / name, root / "evidence" / name)
        # Validator mutation tests synthesize their own adapter identity and evidence.
        # Keep the real shipped ledger's authentic probe from conflicting with that fixture.
        for gate in record["gates"].values():
            if gate["kind"] == "authentic":
                gate["status"] = "not_run"
                gate["evidence"] = []
                gate["prerequisite"] = "This isolated fixture does not exercise this authentic gate."
        record["environment"]["adapter"] = None
        record["acceptance"]["m3_authenticated"] = "not_run"
        self.synthetic_gates = copy.deepcopy(record["gates"])
        return record, root / "m3-results.json", root / "evidence" / name

    def reset_authentic_gate(self, record, gate):
        record["gates"][gate] = copy.deepcopy(self.synthetic_gates[gate])

    def write(self, path, record):
        path.write_text(json.dumps(record))
        return qualification.validate(path)

    @staticmethod
    def entry(root, file):
        return {"path": file.relative_to(root).as_posix(), "sha256": hashlib.sha256(file.read_bytes()).hexdigest()}

    LAUNCH = hashlib.sha256(b"launch of the example adapter").hexdigest()
    ADAPTER = {
        "agent_name": "example-adapter",
        "agent_version": "1.0",
        "protocol_version": 1,
        "fixture_detected": False,
        "probe_status": "Ready",
        "executable_sha256": hashlib.sha256(b"example adapter bytes").hexdigest(),
        "launch_identity": LAUNCH,
        "platform": {"system": "Linux", "machine": "x86_64"},
    }

    @staticmethod
    def measurements(gate):
        """Measurements that satisfy each authentic gate's own contract."""
        step = lambda name, revision: {
            "name": name, "revision": revision, "timeline_identity": "t" * 16, "frame_identity": "f" * 16,
            "audio_identity": "a" * 16, "inventories_checked": True,
        }
        contained = lambda *names: [{"name": n, "escaped_descendants": 0, "group_empty_after": True} for n in names]
        return {
            "auth_adapter_probe_v1": {"auth_status": "ready", "capabilities": {"load_session": True}},
            "auth_two_edit_undo_restart": {"steps": [
                step("brief_result_a", "a" * 40), step("second_edit_b", "b" * 40), step("undo_to_a", "a" * 40), step("restart_recovered", "a" * 40),
            ]},
            "auth_writer_process_group": {"scenarios": contained("edit", "stop", "provider_crash")},
            "auth_compiler_error_repair": {"compiler_error_introduced": True, "repair_context_delivered": True, "repair_attempts": 1},
            "auth_interaction_and_failure_modes": {"scenarios": [
                {"name": n, "passed": True, "stuck_writers": 0}
                for n in ("permission", "clarification", "stop_each_phase", "provider_crash", "source_conflict", "interrupted_publication_restart")
            ]},
            "auth_mcp_cli_support": {"mcp_session_new_accepted": True, "cli_route_ran": False, "project_tool_calls": 3},
            "auth_twenty_cycle_cleanup": {"cycles": 20, "owned_processes_after": 0, "broker_grants_after": 0, "leases_after": 0},
            "auth_visual_ime": {
                "ime": {"name": "ibus-example", "composition_verified": True}, "focus_tab_order_verified": True,
                "scroll_while_streaming_verified": True, "space_not_stolen_verified": True, "session": {"display_server": "wayland"},
            },
            "auth_physical_audio": {"device": {"name": "Built-in output", "physical": True}, "output_clock_measured": True, "handoff_epoch_fresh": True, "display_presented": True},
            "auth_windows": {"workflow_run": True, "packaging_verified": True, "job_object_containment_verified": True},
            "auth_macos": {"workflow_run": True, "packaging_verified": True, "process_containment_verified": True},
        }[gate]

    def authentic_record(self, gate="auth_adapter_probe_v1", **overrides):
        system = {"auth_windows": "Windows", "auth_macos": "Darwin"}.get(gate, "Linux")
        record = {
            "schema": qualification.EVIDENCE_SCHEMA,
            "evidence_kind": "authentic",
            "fixture_only": False,
            "gate": gate,
            "platform": {"system": system, "machine": "x86_64"},
            "adapter": {"agent_name": "example-adapter", "protocol_version": 1, "launch_identity": self.LAUNCH},
            "probe": {"status": "Ready", "protocol_version": 1, "fixture_detected": False},
            "cleanup": {"owned_processes_after": 0},
            "measurements": self.measurements(gate),
        }
        record.update(overrides)
        return record

    def authentic_file(self, directory, gate="auth_adapter_probe_v1", name=None, record=None):
        path = directory / (name or f"{gate}.json")
        path.write_text(json.dumps(record if record is not None else self.authentic_record(gate)))
        return path

    def passing(self, root, record, directory, gate="auth_adapter_probe_v1", file=None):
        """Marks `gate` as passed on the strength of `file` (default: a valid record)."""
        entry = record["gates"][gate]
        entry.pop("prerequisite", None)
        entry["status"] = "pass"
        entry["evidence"] = [self.entry(root, file or self.authentic_file(directory, gate))]
        record["environment"]["adapter"] = dict(self.ADAPTER)
        return entry

    def test_default_cli_validates_every_shipped_ledger(self):
        self.assertEqual(
            [p.name for p in qualification.DEFAULT_LEDGERS],
            [
                "m0-results.json",
                "m2-results.json",
                "m3-results.json",
                "m4-results.json",
                "m5-results.json",
                "m6-results.json",
            ],
        )
        for path in qualification.DEFAULT_LEDGERS:
            qualification.validate(path)

    def test_schema_and_validator_agree_on_gates_and_statuses(self):
        schema = json.loads(self.LEDGER.with_name("m3-results.schema.json").read_text())
        self.assertEqual(set(schema["properties"]["gates"]["required"]), set(qualification.M3_GATES))
        definitions = schema["definitions"]
        for name, kind in qualification.M3_GATES.items():
            reference = schema["properties"]["gates"]["properties"][name]["$ref"]
            self.assertEqual(definitions[reference.rsplit("/", 1)[1]]["properties"]["kind"]["const"], kind)
        self.assertEqual(set(definitions["authenticGate"]["properties"]["status"]["enum"]), qualification.M3_STATUSES)

    def test_shipped_ledger_never_claims_authenticated_acceptance_from_fixtures(self):
        record = qualification.validate(self.LEDGER)
        authentic = [g for g in record["gates"].values() if g["kind"] == "authentic"]
        if record["acceptance"]["m3_authenticated"] == "pass":
            self.assertTrue(all(g["status"] == "pass" for g in authentic))
            self.assertIsNotNone(record["environment"]["adapter"])
        else:
            self.assertTrue(any(g["status"] != "pass" for g in authentic))
        for gate in authentic:
            if gate["status"] == "not_run":
                self.assertEqual(gate["evidence"], [])

    def test_pass_needs_evidence_and_not_run_needs_a_prerequisite(self):
        with tempfile.TemporaryDirectory() as temp:
            record, ledger, _ = self.copied(Path(temp))
            gate = record["gates"]["dev_acp_transport"]
            saved = gate["evidence"]
            gate["evidence"] = []
            with self.assertRaisesRegex(ValueError, "pass without evidence"):
                self.write(ledger, record)
            gate["evidence"] = saved
            other = record["gates"]["auth_windows"]
            prerequisite = other.pop("prerequisite")
            with self.assertRaisesRegex(ValueError, "without a stated prerequisite"):
                self.write(ledger, record)
            other["prerequisite"] = "too short"
            with self.assertRaisesRegex(ValueError, "without a stated prerequisite"):
                self.write(ledger, record)
            other["prerequisite"] = prerequisite
            self.write(ledger, record)

    def test_every_evidence_file_is_hash_checked_and_confined(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            target = directory / record["gates"]["dev_acp_transport"]["evidence"][0]["path"].rsplit("/", 1)[1]
            if not target.is_file():
                target = directory / "logs" / target.name
            target.write_bytes(target.read_bytes() + b"x")
            with self.assertRaisesRegex(ValueError, "Evidence changed"):
                self.write(ledger, record)
            target.write_bytes(target.read_bytes()[:-1])
            self.write(ledger, record)
            record["gates"]["dev_acp_transport"]["evidence"][0] = {"path": "../m0-results.json", "sha256": "0" * 64}
            with self.assertRaisesRegex(ValueError, "escapes the evidence directory"):
                self.write(ledger, record)

    def test_credential_like_text_in_evidence_or_ledger_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            note = directory / "note.log"
            for text in ("api_key = abcdefghijklmnop1234", "Authorization: Bearer abcdefghijklmnopqrstuvwx", "sk-" + "a" * 30, "ghp_" + "b" * 36):
                note.write_text(text)
                record["gates"]["dev_acp_transport"]["evidence"].append(self.entry(root, note))
                with self.assertRaisesRegex(ValueError, "credential-like text"):
                    self.write(ledger, record)
                record["gates"]["dev_acp_transport"]["evidence"].pop()
            record["gates"]["dev_acp_transport"]["notes"] = "token: abcdefghijklmnop1234"
            with self.assertRaisesRegex(ValueError, "credential-like text"):
                self.write(ledger, record)

    def test_an_authentic_pass_cannot_rest_on_development_or_fixture_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            gate = self.passing(root, record, directory, "auth_two_edit_undo_restart")
            # A development measurement (fixture-only) in an authentic gate.
            development = directory / "summary.json"
            development_record = json.loads(development.read_text())
            self.assertEqual(development_record["evidence_kind"], "development")
            self.assertIs(development_record["fixture_only"], True)
            gate["evidence"] = [self.entry(root, development)]
            with self.assertRaisesRegex(ValueError, "contract|development evidence|not authentic"):
                self.write(ledger, record)
            # Logs are not structured authentic evidence either.
            log = next((directory / "logs").glob("*.log"))
            gate["evidence"] = [self.entry(root, log)]
            with self.assertRaisesRegex(ValueError, "not a structured evidence record"):
                self.write(ledger, record)
            # A valid record still needs a real, non-fixture adapter identity.
            gate["evidence"] = [self.entry(root, self.authentic_file(directory, "auth_two_edit_undo_restart"))]
            record["environment"]["adapter"] = None
            with self.assertRaisesRegex(ValueError, "non-fixture adapter identity"):
                self.write(ledger, record)
            for bad in (
                {**self.ADAPTER, "agent_name": "scripted-agent"},
                {**self.ADAPTER, "agent_name": "protocol-peer"},
                {**self.ADAPTER, "fixture_detected": True},
                {**self.ADAPTER, "launch_identity": None},
                {**self.ADAPTER, "launch_identity": "abc"},
                {**self.ADAPTER, "executable_sha256": None},
            ):
                record["environment"]["adapter"] = bad
                with self.assertRaisesRegex(ValueError, "non-fixture adapter identity"):
                    self.write(ledger, record)

    def test_a_marker_only_authentic_record_is_rejected_for_every_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            for gate in qualification.AUTHENTIC_CONTRACTS:
                marker = self.authentic_file(directory, gate, record={"evidence_kind": "authentic", "fixture_only": False})
                self.passing(root, record, directory, gate, marker)
                with self.assertRaisesRegex(ValueError, "does not follow the m3-authentic/1 contract"):
                    self.write(ledger, record)
                self.reset_authentic_gate(record, gate)

    def test_a_real_shaped_record_passes_each_authentic_gate_but_does_not_imply_acceptance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            for gate in qualification.AUTHENTIC_CONTRACTS:
                self.passing(root, record, directory, gate)
                self.write(ledger, record)
                self.reset_authentic_gate(record, gate)
            self.passing(root, record, directory, "auth_adapter_probe_v1")
            record["acceptance"]["m3_authenticated"] = "pass"
            with self.assertRaisesRegex(ValueError, "exactly when every authentic gate passes"):
                self.write(ledger, record)

    def rejects(self, root, record, ledger, directory, gate, message, expected):
        file = self.authentic_file(directory, gate, record=message)
        self.passing(root, record, directory, gate, file)
        with self.assertRaisesRegex(ValueError, expected):
            self.write(ledger, record)

    def test_authentic_records_are_bound_to_gate_platform_launch_probe_and_cleanup(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            base = lambda gate="auth_adapter_probe_v1", **extra: self.authentic_record(gate, **extra)
            gate = "auth_adapter_probe_v1"
            cases = [
                ("fixture_only absent", {k: v for k, v in base().items() if k != "fixture_only"}, "fixture_only: false"),
                ("fixture_only true", base(fixture_only=True), "fixture_only: false"),
                ("development kind", base(evidence_kind="development"), "not authentic"),
                ("another gate", base("auth_mcp_cli_support"), "bound to another gate"),
                ("no platform", {k: v for k, v in base().items() if k != "platform"}, "platform it was measured on"),
                ("failed probe", base(probe={"status": "AuthRequired", "protocol_version": 1, "fixture_detected": False}), "successful, non-fixture probe"),
                ("fixture probe", base(probe={"status": "Ready", "protocol_version": 1, "fixture_detected": True}), "successful, non-fixture probe"),
                ("another adapter", base(adapter={"agent_name": "other", "protocol_version": 1, "launch_identity": self.LAUNCH}), "another adapter than the ledger"),
                ("another launch", base(adapter={"agent_name": "example-adapter", "protocol_version": 1, "launch_identity": "0" * 64}), "another adapter launch"),
                ("changed arguments", base(adapter={"agent_name": "example-adapter", "protocol_version": 1, "launch_identity": qualification.launch_identity_digest("0" * 64, ["--changed"], [])}), "another adapter launch"),
                ("old protocol", base(adapter={"agent_name": "example-adapter", "protocol_version": 2, "launch_identity": self.LAUNCH}), "did not negotiate ACP v1"),
                ("unclean teardown", base(cleanup={"owned_processes_after": 1}), "clean teardown"),
                ("no cleanup", {k: v for k, v in base().items() if k != "cleanup"}, "clean teardown"),
                ("no measurements", {k: v for k, v in base().items() if k != "measurements"}, "no measurements"),
                ("auth not ready", base(measurements={"auth_status": "authrequired", "capabilities": {"a": 1}}), "authentication was not proven ready"),
            ]
            for label, bad, expected in cases:
                with self.subTest(label):
                    self.rejects(root, record, ledger, directory, gate, bad, expected)
                    self.reset_authentic_gate(record, gate)
            # Platform gates need their own platform.
            for gate_name, wrong in (("auth_windows", "Linux"), ("auth_macos", "Windows")):
                with self.subTest(gate_name):
                    bad = self.authentic_record(gate_name, platform={"system": wrong, "machine": "x86_64"})
                    self.rejects(root, record, ledger, directory, gate_name, bad, "measured on")
                    self.reset_authentic_gate(record, gate_name)

    def test_each_gates_own_measurements_are_enforced(self):
        defects = {
            "auth_two_edit_undo_restart": lambda m: m["steps"][2].update(revision="c" * 40),
            "auth_writer_process_group": lambda m: m["scenarios"][1].update(escaped_descendants=1),
            "auth_compiler_error_repair": lambda m: m.update(repair_attempts=2),
            "auth_interaction_and_failure_modes": lambda m: m["scenarios"].pop(),
            "auth_mcp_cli_support": lambda m: m.update(mcp_session_new_accepted=False, project_tool_calls=3),
            "auth_twenty_cycle_cleanup": lambda m: m.update(cycles=19),
            "auth_visual_ime": lambda m: m["ime"].update(composition_verified=False),
            "auth_physical_audio": lambda m: m["device"].update(physical=False),
            "auth_windows": lambda m: m.update(job_object_containment_verified=False),
            "auth_macos": lambda m: m.update(packaging_verified=False),
        }
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            for gate, defect in defects.items():
                with self.subTest(gate):
                    bad = self.authentic_record(gate)
                    defect(bad["measurements"])
                    self.rejects(root, record, ledger, directory, gate, bad, "Authentic pass")
                    self.reset_authentic_gate(record, gate)

    def test_one_record_cannot_back_two_gates_and_not_run_cites_nothing(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            shared = self.authentic_file(directory, "auth_adapter_probe_v1")
            self.passing(root, record, directory, "auth_adapter_probe_v1", shared)
            self.passing(root, record, directory, "auth_writer_process_group", shared)
            with self.assertRaisesRegex(ValueError, "bound to another gate"):
                self.write(ledger, record)
            self.reset_authentic_gate(record, "auth_writer_process_group")
            # The same bytes under two gates are caught as shared evidence too.
            record["environment"]["adapter"] = dict(self.ADAPTER)
            other = record["gates"]["auth_adapter_probe_v1"]
            other["evidence"] = [self.entry(root, shared)]
            ghost = record["gates"]["auth_windows"]
            ghost["evidence"] = [self.entry(root, shared)]
            with self.assertRaisesRegex(ValueError, "not_run but cites evidence"):
                self.write(ledger, record)

    def test_development_passes_must_be_backed_by_the_recorded_harness_run(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            run_path = directory / "harness-run.json"
            run = json.loads(run_path.read_text())
            gate = record["gates"]["dev_acp_transport"]
            original = json.loads(run_path.read_text())

            def rewrite(mutator):
                changed = json.loads(json.dumps(original))
                mutator(changed)
                run_path.write_text(json.dumps(changed))
                for other in record["gates"].values():
                    for item in other["evidence"]:
                        if item["path"].endswith("harness-run.json"):
                            item["sha256"] = hashlib.sha256(run_path.read_bytes()).hexdigest()

            check = lambda changed: next(c for c in changed["checks"] if c["name"] == "acp_transport")
            for label, mutator, expected in (
                ("check failed", lambda c: check(c).update(status="fail"), "did not pass in the recorded run"),
                ("exit code", lambda c: check(c).update(returncode=101), "did not pass in the recorded run"),
                ("no tests", lambda c: check(c).update(test_results=[]), "no passing test results"),
                ("a failing target", lambda c: check(c)["test_results"][0].update(failed=1), "no passing test results"),
                ("check absent", lambda c: c["checks"].remove(check(c)), "did not run check acp_transport"),
                ("survivor", lambda c: c.update(owned_processes_alive_at_end=[123]), "left harness-owned processes alive"),
            ):
                with self.subTest(label):
                    rewrite(mutator)
                    with self.assertRaisesRegex(ValueError, expected):
                        self.write(ledger, record)
            rewrite(lambda c: None)
            self.write(ledger, record)
            # A development pass that does not cite the run record at all is refused.
            gate["evidence"] = [item for item in gate["evidence"] if not item["path"].endswith("harness-run.json")]
            with self.assertRaisesRegex(ValueError, "must cite the harness run record"):
                self.write(ledger, record)
            self.assertEqual(run["checks"][0]["name"], original["checks"][0]["name"])

    def test_private_paths_are_rejected_in_evidence_and_ledger_but_placeholders_and_system_paths_are_not(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            note = directory / "paths.log"
            for text in ("at /root/fframes-desktop/desktop/app", "in /home/alice/work", "/Users/bob/Library", "C:\\Users\\carol", "D:/builds/x", "/tmp/fft-x11abc/video", "cwd=/var/lib/x"):
                note.write_text(text)
                record["gates"]["dev_acp_transport"]["evidence"].append(self.entry(root, note))
                with self.assertRaisesRegex(ValueError, "absolute private path"):
                    self.write(ledger, record)
                record["gates"]["dev_acp_transport"]["evidence"].pop()
            note.write_text("<repo>/desktop/app <home>/.cargo/registry /usr/bin/Xvfb /dev/vda2 https://example.com/root/page http://host/tmp/x")
            record["gates"]["dev_acp_transport"]["evidence"].append(self.entry(root, note))
            self.write(ledger, record)
            record["gates"]["dev_acp_transport"]["evidence"].pop()
            record["gates"]["dev_acp_transport"]["notes"] = "ran in /home/alice/video"
            with self.assertRaisesRegex(ValueError, "absolute private path"):
                self.write(ledger, record)

    def test_a_nul_byte_never_disables_the_credential_scan(self):
        key = "sk-" + "c" * 30
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            gate = record["gates"]["dev_acp_transport"]
            evidence = directory / "binary.log"
            for label, data, expected in (
                ("nul then key in a log", b"\0" + key.encode(), "unexpected binary content"),
                ("key then nul", key.encode() + b"\0tail", "unexpected binary content"),
                ("utf-16 key", key.encode("utf-16-le"), "unexpected binary content"),
            ):
                with self.subTest(label):
                    evidence.write_bytes(data)
                    gate["evidence"].append(self.entry(root, evidence))
                    with self.assertRaisesRegex(ValueError, expected):
                        self.write(ledger, record)
                    gate["evidence"].pop()
            # The scan itself (used for any permitted binary) looks through NULs.
            png = directory / "shot.png"
            png.write_bytes(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRtEXt" + b"comment\0" + key.encode() + b"\0\0")
            with self.assertRaisesRegex(ValueError, "credential-like text"):
                qualification.scan_for_secrets(png, "image")
            png.write_bytes(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0")
            qualification.scan_for_secrets(png, "image")
            utf16 = directory / "wide.png"
            utf16.write_bytes(key.encode("utf-16-le"))
            with self.assertRaisesRegex(ValueError, "credential-like text"):
                qualification.scan_for_secrets(utf16, "image")
            odd = directory / "evidence.bin"
            odd.write_bytes(b"x")
            with self.assertRaisesRegex(ValueError, "unsupported evidence type"):
                qualification.scan_for_secrets(odd, "evidence")
            text = directory / "latin.log"
            text.write_bytes(b"caf\xe9")
            with self.assertRaisesRegex(ValueError, "not valid UTF-8"):
                qualification.scan_for_secrets(text, "evidence")

    def test_the_launch_identity_digest_matches_the_applications_vector(self):
        # The same vector is asserted by the application (conversation_panel/qualification.rs).
        digest = qualification.launch_identity_digest("aa" * 32, ["--acp", "x y"], ["B", "A"], [(0, "bb" * 32)])
        self.assertEqual(digest, "819ef19d8b35aa9874b2025b70414b64535eed3d11429bfaf1f342e5fabbdb65")
        self.assertNotEqual(digest, qualification.launch_identity_digest("aa" * 32, ["--acp", "x y"], ["B", "A"], []))
        self.assertEqual(digest, qualification.launch_identity_digest("aa" * 32, ["--acp", "x y"], ["A", "B"], [(0, "bb" * 32)]))

    def test_a_development_gate_cannot_cite_authentic_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            record, ledger, directory = self.copied(root)
            record["gates"]["dev_acp_transport"]["evidence"].append(self.entry(root, self.authentic_file(directory)))
            with self.assertRaisesRegex(ValueError, "cites authentic evidence"):
                self.write(ledger, record)


class HarnessTests(unittest.TestCase):
    """Safety properties of desktop/scripts/qualify-m3-agent.py that need no cargo run."""

    def test_redaction_removes_configured_values_and_credential_patterns(self):
        secret = "zQ9-plain-credential-0042"
        redactor = harness.Redactor([secret])
        text = f"auth {secret} and api_key = abcdefghijklmnop1234 and sk-{'a' * 30}"
        clean = redactor.text(text)
        self.assertNotIn(secret, clean)
        self.assertNotIn("abcdefghijklmnop1234", clean)
        self.assertNotIn("sk-aaaa", clean)
        nested = redactor.json({secret: [secret, {"k": f"Bearer {'b' * 24}"}], "n": 3})
        self.assertNotIn(secret, json.dumps(nested))
        self.assertNotIn("bbbbbbbb", json.dumps(nested))
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "evidence.json"
            path.write_text(json.dumps(nested))
            qualification.scan_for_secrets(path, "redacted output")

    def test_private_absolute_paths_are_normalized_before_serialization(self):
        with tempfile.TemporaryDirectory() as temp:
            work = Path(temp) / "fm3-work"
            work.mkdir()
            sdk = Path(temp) / "stage-home/.fframes/sdk/active"
            sdk.mkdir(parents=True)
            redactor = harness.Redactor([], harness.path_roots(sdk))
            redactor.add_roots([(work, "<work>")])
            repo = harness.ROOT
            text = (
                f"{sdk}/compatibility.json {work}/probe {repo}/desktop/app/src/lib.rs "
                f"{Path.home()}/.cargo/registry/src/x/y.rs /home/alice/private/notes /Users/bob/Library/x "
                "C:\\Users\\carol\\x /usr/bin/Xvfb /dev/vda2"
            )
            clean = redactor.text(text)
            self.assertIn("<sdk-active>/compatibility.json", clean)
            self.assertIn("<work>/probe", clean)
            self.assertIn("<repo>/desktop/app/src/lib.rs", clean)
            self.assertIn("<private-path>", clean)
            self.assertIn("/usr/bin/Xvfb", clean, "system paths identify a platform, not a person")
            for private in (str(repo), str(Path.home()), "alice", "bob", "carol", str(sdk.parents[2])):
                self.assertNotIn(private, clean)
            nested = redactor.json({"cwd": str(repo / "desktop"), "args": ["--real-sdk-active", str(sdk)], "n": 1})
            path = Path(temp) / "evidence.json"
            path.write_text(json.dumps(nested))
            qualification.scan_for_secrets(path, "normalized output")
            self.assertEqual(nested["args"][1], "<sdk-active>")

    def test_measurement_files_written_by_the_tests_are_normalized_too(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "evidence"
            out.mkdir()
            (out / "m.json").write_text(json.dumps({"project": f"{harness.ROOT}/desktop/x", "tmp": "/tmp/fft-abc/data", "n": 1}))
            (out / "clean.json").write_text(json.dumps({"n": 2}))
            redactor = harness.Redactor([], harness.path_roots(None))
            self.assertEqual(harness.normalize_measurements(out, redactor), ["m.json"])
            value = json.loads((out / "m.json").read_text())
            self.assertEqual(value["project"], "<repo>/desktop/x")
            self.assertEqual(value["n"], 1)
            qualification.scan_for_secrets(out / "m.json", "normalized measurement")

    def test_repository_fixtures_are_never_accepted_as_an_adapter(self):
        for name in ("acp-agent.py", "acp-peer.py"):
            fixture = next((assembly.ROOT / "desktop").rglob(name))
            self.assertIsNotNone(harness.looks_like_fixture(str(fixture), []))
            self.assertIsNotNone(harness.looks_like_fixture("python3", [str(fixture)]))
        with tempfile.TemporaryDirectory() as temp:
            declared = Path(temp) / "adapter.py"
            declared.write_text('"""TEST FIXTURE ONLY"""\n')
            self.assertIsNotNone(harness.looks_like_fixture(str(declared), []))
            other = Path(temp) / "other.py"
            other.write_text("print('hi')\n")
            self.assertIsNone(harness.looks_like_fixture(str(other), []))
        args = argparse.Namespace(adapter=str(next((assembly.ROOT / "desktop").rglob("acp-agent.py"))), adapter_arg=[])
        with self.assertRaisesRegex(harness.HarnessError, "fixture"):
            harness.preflight_adapter(args)

    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "requires Unix domain sockets")
    def test_an_occupied_endpoint_is_refused_and_left_alone(self):
        with tempfile.TemporaryDirectory() as temp:
            occupied = Path(temp) / "guard.sock"
            holder = socket.socket(socket.AF_UNIX)
            holder.bind(str(occupied))
            holder.listen(1)
            guard = harness.EndpointGuard()
            guard.path = occupied
            with self.assertRaisesRegex(harness.HarnessError, "occupied"):
                guard.acquire()
            self.assertTrue(occupied.exists(), "a foreign endpoint must not be removed")
            holder.close()
            occupied.unlink()
            guard.acquire()
            self.assertTrue(occupied.exists())
            guard.release()
            self.assertFalse(occupied.exists())

    @unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux process groups and /proc")
    def test_only_harness_owned_processes_are_ever_stopped(self):
        owned = harness.Owned("test-run", harness.Redactor([]))
        with self.assertRaisesRegex(harness.HarnessError, "not harness-owned"):
            owned.stop_group(os.getpgrp())
        bystander = subprocess.Popen(["sleep", "60"], start_new_session=True)
        try:
            with tempfile.TemporaryDirectory() as temp:
                # A setsid child escapes the process group but keeps the harness marker.
                record = owned.run(
                    "escape", ["sh", "-c", "setsid sleep 60 & echo $! > escaped.pid; sleep 0.3"],
                    cwd=Path(temp), env={"PATH": os.environ["PATH"]}, output=Path(temp) / "out.log", timeout=30,
                )
                escaped = int((Path(temp) / "escaped.pid").read_text())
            self.assertIn(escaped, [item["pid"] for item in record["leftovers"]])
            deadline = time.monotonic() + 5
            while Path(f"/proc/{escaped}").exists() and not harness.is_zombie(escaped) and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(Path(f"/proc/{escaped}").exists() is False or harness.is_zombie(escaped))
            self.assertIsNone(bystander.poll(), "an unrelated process was signalled")
            self.assertEqual(owned.shutdown(), [])
        finally:
            bystander.kill()
            bystander.wait()

    class FakeTable(harness.ProcFs):
        """A process table with reusable identifiers. Delivery is recorded, never real."""

        def __init__(self):
            self.procs: dict[int, dict] = {}
            self.sent: list[tuple[int, int]] = []

        def put(self, pid, *, pgrp, start, marked=False, state="S"):
            self.procs[pid] = {"pgrp": pgrp, "start": start, "marked": marked, "state": state}

        def pids(self):
            return sorted(self.procs)

        def stat(self, pid):
            p = self.procs.get(pid)
            return (p["state"], 1, p["pgrp"], p["start"]) if p else None

        def has_marker(self, pid, marker):
            return bool(self.procs.get(pid, {}).get("marked"))

        def command(self, pid):
            return f"fake-{pid}"

        def pin(self, pid):
            return None

        def deliver(self, pid, sig, pinned):
            self.sent.append((pid, sig))
            return True

    def test_a_reused_process_group_number_is_never_signalled(self):
        table = self.FakeTable()
        owned = harness.Owned("run", harness.Redactor([]), table)
        # The harness spawned group 100 (leader pid 100); everything in it later exits.
        table.put(100, pgrp=100, start=5, marked=True)
        owned.adopt(100)
        self.assertEqual(owned.groups[100], harness.Identity(100, 5))
        del table.procs[100]
        self.assertTrue(owned.retire_if_empty(100), "an empty group is retired")
        self.assertNotIn(100, owned.groups)
        # An unrelated process later gets pgid 100: it is not ours, by any route.
        table.put(200, pgrp=100, start=900)
        with self.assertRaisesRegex(harness.HarnessError, "not harness-owned"):
            owned.stop_group(100)
        self.assertEqual(owned.reap_leftovers(100), [])
        self.assertEqual(owned.shutdown(), [])
        self.assertEqual(table.sent, [], "a retired group number must never be signalled")

    @unittest.skipUnless(hasattr(signal, "SIGKILL"), "requires POSIX SIGKILL")
    def test_a_stale_group_entry_whose_number_was_reused_signals_nothing_unproven(self):
        table = self.FakeTable()
        owned = harness.Owned("run", harness.Redactor([]), table)
        table.put(100, pgrp=100, start=5, marked=True)
        owned.adopt(100)
        # The leader exits without the harness noticing; pid/pgid 100 is reused by a stranger
        # (new start time, no marker) that now leads its own group of two.
        table.put(100, pgrp=100, start=777)
        table.put(101, pgrp=100, start=778)
        members = owned.owned_members(100)
        self.assertEqual(members, [], "neither the stranger nor its child is proven ours")
        self.assertEqual({item["pid"] for item in owned.unowned_members}, {100, 101})
        self.assertEqual(owned.reap_leftovers(100), [])
        self.assertEqual(table.sent, [])
        # A marked member of the group (a real descendant) is still ours and is signalled.
        table.put(102, pgrp=100, start=779, marked=True)
        self.assertEqual([m.pid for m in owned.owned_members(100)], [102])
        left = owned.reap_leftovers(100)
        self.assertEqual([item["pid"] for item in left], [102])
        self.assertEqual(table.sent, [(102, signal_kill())])

    def test_an_owned_pid_reused_between_discovery_and_signal_is_not_signalled(self):
        table = self.FakeTable()
        owned = harness.Owned("run", harness.Redactor([]), table)
        table.put(300, pgrp=300, start=7, marked=True)
        (identity,) = owned.marked_identities()
        self.assertEqual(identity, harness.Identity(300, 7))
        # The process exits and the number is reused (even by something carrying the marker
        # through an inherited environment): a different start time is a different process.
        table.put(300, pgrp=300, start=8, marked=True)
        self.assertFalse(table.signal(identity, signal_kill(), owned.marker))
        self.assertEqual(table.sent, [])
        # An unmarked process with the same identity is not ours either.
        table.put(300, pgrp=300, start=7, marked=False)
        self.assertFalse(table.signal(identity, signal_kill(), owned.marker))
        # The very process that was observed, still marked, is.
        table.put(300, pgrp=300, start=7, marked=True)
        self.assertTrue(table.signal(identity, signal_kill(), owned.marker))
        self.assertEqual(table.sent, [(300, signal_kill())])

    def test_the_x11_shell_check_is_never_a_pass_without_sdk_and_x11_tools(self):
        check = harness.CHECKS_BY_NAME["native_x11_shell"]
        self.assertEqual(check.gates, ["dev_native_x11_shell"])
        self.assertEqual(check.needs_tools, ["Xvfb", "xdotool"])
        self.assertIn("--ignored", check.argv)
        self.assertIn("x11_shell", check.argv)
        self.assertEqual(harness.CHECKS_BY_NAME["app_ui_workflow"].targets, ["tests/agent_workflow_ui.rs"])
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(cargo_timeout=1.0)
            runner = harness.Runner(args, Path(temp), Path(temp), harness.Owned("t", harness.Redactor([])), harness.Redactor([]), {})
            without_sdk = runner.run(check, True, None)
            self.assertEqual(without_sdk.status, "not_run")
            self.assertIn("--real-sdk-active", without_sdk.prerequisite)
            real_which = shutil.which
            shutil.which = lambda name, *a, **k: None if name in ("Xvfb", "xdotool") else real_which(name, *a, **k)
            try:
                without_tools = runner.run(check, True, Path(temp))
            finally:
                shutil.which = real_which
            self.assertEqual(without_tools.status, "not_run")
            self.assertIn("Xvfb", without_tools.prerequisite)
            self.assertIn("xdotool", without_tools.prerequisite)
            self.assertEqual(runner.owned.history, [], "nothing may be launched when a prerequisite is missing")

    def test_cargo_output_is_parsed_per_test_binary(self):
        text = (
            "   Running tests/a.rs (target/debug/deps/a-1)\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured\n"
            "   Running tests/b.rs (target/debug/deps/b-2)\ntest result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured\n"
        )
        results = harness.parse_cargo_output(text)["results"]
        self.assertEqual([(r["target"], r["outcome"], r["passed"], r["failed"]) for r in results], [("tests/a.rs", "ok", 3, 0), ("tests/b.rs", "FAILED", 1, 2)])

    def test_help_documents_every_argument(self):
        parser_help = subprocess.run(["python3", str(Path(harness.__file__)), "--help"], capture_output=True, text=True, check=True).stdout
        for flag in ("--out", "--date", "--ledger", "--replace", "--checks", "--list-checks", "--cargo-timeout", "--real-sdk-active", "--adapter", "--adapter-arg", "--auth-env"):
            self.assertIn(flag, parser_help)
            self.assertIn(flag, harness.__doc__)


if __name__ == "__main__":
    unittest.main()
