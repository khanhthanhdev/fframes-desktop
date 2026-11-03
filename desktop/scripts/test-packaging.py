#!/usr/bin/env python3
"""Regression checks for artifact integrity, packaging and release publication."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
import zipfile


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
windows_installer = load("package-windows-installer")
github_release = load("prepare-github-release")


class ArtifactTests(unittest.TestCase):
    def test_sdk_framework_keeps_registry_versions_instead_of_development_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            framework = Path(temporary)
            for crate in ["fframes-media", "fframes-skia-renderer"]:
                (framework / crate).mkdir()
                shutil.copy(assembly.ROOT / crate / "Cargo.toml", framework / crate / "Cargo.toml")
            source = '''[workspace]
lints = {}
[workspace.dependencies]
svgr = {path = "../svgr/crates/svgr", version = "0.50.2", default-features = false}
usvgr = {path = "../svgr/crates/usvgr", version = "0.50.2", features = ["text"]}
fframes = {path = "fframes", version = "1.3.0"}
ffmpeg-sys-fframes = "9.0.0"
'''
            assembly.write_framework(framework, source)
            dependencies = tomllib.loads((framework / "Cargo.toml").read_text())["workspace"]["dependencies"]
            self.assertEqual(dependencies["svgr"], {"version": "0.50.2", "default-features": False})
            self.assertEqual(dependencies["usvgr"], {"version": "0.50.2", "features": ["text"]})
            self.assertEqual(dependencies["fframes"]["path"], "fframes")
            self.assertEqual(dependencies["ffmpeg-sys-fframes"], "9.0.0")

    def test_sdk_manifest_has_stable_digest_and_no_preview_fields(self):
        raw = json.loads((assembly.ROOT / "desktop/packaging/sdk/phase-zero-sdk.json").read_text())
        self.assertNotIn("preview_contract_versions", raw)
        canonical = json.dumps(raw, separators=(",", ":"))
        self.assertEqual(__import__("hashlib").sha256(canonical.encode()).hexdigest(), "3c80a0f2d43cec6a3239a14520251159bed81d4bddb548548baf44d9db95ddff")
        fframes_version = tomllib.loads((assembly.ROOT / "fframes/Cargo.toml").read_text())["package"]["version"]
        cargo_fframes_version = tomllib.loads((assembly.ROOT / "cargo-fframes/Cargo.toml").read_text())["package"]["version"]
        self.assertEqual(raw["fframes_version"], fframes_version)
        self.assertEqual(raw["cargo_fframes_version"], cargo_fframes_version)
        runtime_version = tomllib.loads((assembly.ROOT / "fframes-studio-runtime/Cargo.toml").read_text())["package"]["version"]
        protocol_version = tomllib.loads((assembly.ROOT / "fframes-studio-protocol/Cargo.toml").read_text())["package"]["version"]
        self.assertEqual(raw["runtime_version"], runtime_version)
        self.assertEqual(protocol_version, runtime_version)
        fixture = assembly.ROOT / "desktop/fixtures/annotated-video-overlay"
        fixture_manifest = tomllib.loads((fixture / "Cargo.toml").read_text())
        fixture_lock = tomllib.loads((fixture / "Cargo.lock").read_text())
        locked_fframes = next(package for package in fixture_lock["package"] if package["name"] == "fframes")
        self.assertEqual(fixture_manifest["dependencies"]["fframes"]["version"], fframes_version)
        self.assertEqual(locked_fframes["version"], fframes_version)
        self.assertIn(f'"{fframes_version}"', (fixture / "src/main.rs").read_text())

    def test_packaged_launchers_start_the_studio_workspace(self):
        packager = (assembly.ROOT / "desktop/scripts/package-phase-zero.py").read_text()
        self.assertIn('"CFBundleIdentifier": "studio.fframes.app"', packager)
        self.assertIn('bin/fframes-studio.exe") studio', packager)
        self.assertIn('fframes-studio.exe" studio %*', packager)
        self.assertIn('bin/fframes-studio" studio', packager)
        self.assertIn('MacOS/fframes-studio" studio', packager)

    def fake_windows_package(self, root):
        source = root / "fframes-studio-x86_64-pc-windows-msvc"
        for name in windows_installer.REQUIRED:
            (source / name).parent.mkdir(parents=True, exist_ok=True)
            (source / name).write_text(name)
        return source

    def test_windows_installer_is_per_user_and_keeps_user_data(self):
        script = windows_installer.SCRIPT.read_text()
        setup = dict(
            line.split("=", 1)
            for line in script.split("[Setup]", 1)[1].split("\n[", 1)[0].splitlines()
            if "=" in line and not line.startswith(";") and not line.startswith("#")
        )
        self.assertEqual(setup["PrivilegesRequired"], "lowest", "the installer must not need administrator rights")
        self.assertNotIn("PrivilegesRequiredOverridesAllowed", setup)
        # {autopf} is %LOCALAPPDATA%\Programs for a per-user install: outside the app data root.
        self.assertEqual(setup["DefaultDirName"], r"{autopf}\fframes Studio")
        self.assertEqual(setup["AppId"], "{{A4FDB3DF-A64F-4AD1-AA24-245DE70251EC}", "AppId identifies upgrades; never change it")
        self.assertNotIn("[UninstallDelete]", script, "uninstall must keep projects, the SDK and app state")
        self.assertNotIn("{localappdata}", script.lower())
        self.assertNotIn("{%userprofile}", script.lower())
        for shortcut in ("{autoprograms}", "{autodesktop}"):
            line = next(line for line in script.splitlines() if line.startswith(f'Name: "{shortcut}'))
            self.assertIn(r'Filename: "{app}\bin\fframes-studio.exe"', line)
        self.assertTrue((windows_installer.SCRIPT.parent / "fframes-studio.ico").is_file())
        # The installed shortcut passes no SDK variable, so the app must find sdk\ beside bin\.
        sdk_setup = (assembly.ROOT / "desktop/crates/studio-engine/src/sdk_setup.rs").read_text()
        self.assertIn('p.parent()?.parent()?.join("sdk")', sdk_setup)
        main = (assembly.ROOT / "desktop/app/src/main.rs").read_text()
        self.assertIn('#![cfg_attr(windows, windows_subsystem = "windows")]', main)
        # The shortcut passes no command: that must open the studio.
        self.assertIn("None => run_studio(None),", main)

    def test_windows_installer_rejects_incomplete_packages_before_compiling(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = self.fake_windows_package(root)
            (source / "sdk/compatibility.json").unlink()
            with self.assertRaisesRegex(ValueError, "missing: sdk/compatibility.json"):
                windows_installer.package(source, root / "setup.exe", iscc=str(root / "absent.exe"))
            existing = root / "existing.exe"
            existing.write_bytes(b"previous")
            with self.assertRaisesRegex(ValueError, "output already exists"):
                windows_installer.package(source, existing)
            self.assertEqual(existing.read_bytes(), b"previous")
            with self.assertRaisesRegex(ValueError, "must be an .exe"):
                windows_installer.package(source, root / "setup.msi")
        self.assertEqual(windows_installer.app_version()[1].count("."), 3)

    @unittest.skipUnless(os.environ.get("INNO_SETUP_COMPILER"), "set INNO_SETUP_COMPILER to the pinned ISCC.exe")
    def test_windows_installer_compiles_with_the_pinned_inno_setup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = self.fake_windows_package(root)
            (source / "launch.bat").write_text("excluded")
            output = windows_installer.package(source, root / "out/fframes-studio-setup.exe")
            self.assertEqual(output.read_bytes()[:2], b"MZ")
            self.assertEqual([path.name for path in output.parent.iterdir()], [output.name], "no temporary output is left behind")
            with self.assertRaisesRegex(ValueError, "output already exists"):
                windows_installer.package(source, output)

    def test_preview_release_carries_matching_manifests_and_checksums(self):
        version = tomllib.loads((assembly.ROOT / "desktop/app/Cargo.toml").read_text())["package"]["version"]
        tag = f"v{version}"
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            (dist / github_release.SETUP).write_bytes(b"MZ setup")
            (dist / github_release.WINDOWS_ZIP).write_bytes(b"PK windows")
            (dist / "fframes-studio-x86_64-unknown-linux-gnu.deb").write_bytes(b"deb")
            with self.assertRaisesRegex(ValueError, "does not match the app version"):
                github_release.prepare("v99.0.0", dist)
            github_release.prepare(tag, dist, "someone/fork")

            setup_sha = github_release.sha256(dist / github_release.SETUP)
            zip_sha = github_release.sha256(dist / github_release.WINDOWS_ZIP)
            installer = (dist / "khanhthanhdev.fframesStudio.installer.yaml").read_text()
            self.assertIn(f"PackageVersion: {version}\n", installer)
            self.assertIn("InstallerType: inno\nScope: user\n", installer)
            self.assertIn(f"InstallerSha256: {setup_sha.upper()}\n", installer)
            self.assertIn(f"https://github.com/someone/fork/releases/download/{tag}/{github_release.SETUP}", installer)
            # ProductCode is how winget recognises the installed app; it must be the installer's AppId.
            self.assertIn("ProductCode: '{A4FDB3DF-A64F-4AD1-AA24-245DE70251EC}_is1'", installer)
            for name in ("khanhthanhdev.fframesStudio.yaml", "khanhthanhdev.fframesStudio.locale.en-US.yaml"):
                self.assertIn("ManifestVersion: 1.12.0\n", (dist / name).read_text())

            scoop = json.loads((dist / github_release.SCOOP_MANIFEST).read_text())
            self.assertEqual(scoop["version"], version)
            self.assertEqual(scoop["architecture"]["64bit"]["hash"], zip_sha)
            self.assertEqual(scoop["architecture"]["64bit"]["extract_dir"], "fframes-studio-x86_64-pc-windows-msvc")
            self.assertEqual(scoop["shortcuts"], [["bin\\fframes-studio.exe", "fframes Studio"]])

            script = (dist / "install.ps1").read_bytes()
            self.assertIn(b'[string]$Repository = "someone/fork"', script)
            self.assertNotIn(github_release.DEFAULT_REPOSITORY.encode(), script)
            script.decode("ascii")

            sums = dict(reversed(line.split("  ", 1)) for line in (dist / "SHA256SUMS.txt").read_text().splitlines())
            published = sorted(path.name for path in dist.iterdir() if path.name not in {"SHA256SUMS.txt", "release-notes.md"})
            self.assertEqual(sorted(sums), published)
            for name, digest in sums.items():
                self.assertEqual(github_release.sha256(dist / name), digest)
            notes = (dist / "release-notes.md").read_text()
            self.assertIn("irm https://github.com/someone/fork/releases/latest/download/install.ps1 | iex", notes)
            self.assertIn("not code-signed", notes)
            with self.assertRaisesRegex(ValueError, "already contains generated files"):
                github_release.prepare(tag, dist, "someone/fork")

    def test_install_script_verifies_the_download_before_running_setup(self):
        script = github_release.INSTALL_SCRIPT.read_text()
        self.assertIn(f'$setupName = "{github_release.SETUP}"', script)
        self.assertLess(script.index("Get-FileHash"), script.index("Start-Process -FilePath $setupPath"))
        self.assertIn("{A4FDB3DF-A64F-4AD1-AA24-245DE70251EC}_is1", script)
        # Piped through Invoke-Expression, `exit` would close the user's shell.
        self.assertNotRegex(script, r"(?m)^\s*exit\b")

    def test_preview_publication_is_separate_from_the_gated_consumer_release(self):
        workflow = (assembly.ROOT / ".github/workflows/desktop-release.yml").read_text()
        preview = workflow.split("\n  publish-preview:\n", 1)[1].split("\n  publish:\n", 1)[0]
        consumer = workflow.split("\n  publish:\n", 1)[1]
        self.assertIn("prepare-github-release.py", preview)
        self.assertIn("(unsigned preview)", preview)
        self.assertIn("published bytes are immutable", preview)
        self.assertNotIn("release-gate.py", preview)
        self.assertIn("release-gate.py", consumer)
        self.assertLess(
            workflow.index("Check the release tag matches the app version"),
            workflow.index("Focused tests and lint"),
        )

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

    def test_bundled_skia_never_compiles_and_ships_no_dev_dependencies(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            shutil.copytree(assembly.ROOT / "fframes-skia-renderer", root / "fframes-skia-renderer", ignore=shutil.ignore_patterns("target"))
            assembly.configure_bundled_skia(root)
            manifest = tomllib.loads((root / "fframes-skia-renderer/Cargo.toml").read_text())
            self.assertIn("no-compile", manifest["dependencies"]["skia-safe"]["features"])
            self.assertNotIn("dev-dependencies", manifest)
            self.assertIn("features", manifest)
            self.assertFalse((root / "fframes-skia-renderer/examples").exists())

    def test_every_target_has_a_reviewed_skia_archive_and_a_mismatch_is_refused(self):
        pins = json.loads((assembly.ROOT / assembly.SKIA_PINS).read_text())["archives"]
        self.assertEqual(set(pins), {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"})
        for target, pin in pins.items():
            self.assertTrue(pin["key"].split("-", 1)[1].startswith(target), target)
            self.assertRegex(pin["sha256"], "^[0-9a-f]{64}$")
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / "cache"
            cache.mkdir()
            pin = pins["x86_64-pc-windows-msvc"]
            (cache / f"skia-binaries-{pin['key']}.tar.gz").write_bytes(b"not the reviewed archive")
            previous = os.environ.get("SKIA_ARCHIVE_DIR")
            os.environ["SKIA_ARCHIVE_DIR"] = str(cache)
            try:
                with self.assertRaisesRegex(ValueError, "does not match"):
                    assembly.stage_skia("x86_64-pc-windows-msvc", Path(temp) / "skia")
            finally:
                if previous is None:
                    del os.environ["SKIA_ARCHIVE_DIR"]
                else:
                    os.environ["SKIA_ARCHIVE_DIR"] = previous

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

    def test_the_package_record_holds_the_build_facts_and_no_ledger(self):
        record = packaging.package_record("x86_64-unknown-linux-gnu", True, 12.5)
        self.assertEqual(record["target_platform"]["triple"], "x86_64-unknown-linux-gnu")
        self.assertEqual(record["target_platform"]["arch"], "x86_64")
        self.assertIs(record["build_evidence"]["sdk_included"], True)
        self.assertEqual(record["metrics"]["native_package_build_seconds"], 12.5)
        self.assertNotIn("gates", record)
        packager = (assembly.ROOT / "desktop/scripts/package-phase-zero.py").read_text()
        self.assertNotIn("desktop/qualification", packager)

    def test_finalizer_rehashes_signed_files_and_replaces_the_archive(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            package = root / "fframes-studio-x86_64-pc-windows-msvc"
            binary = package / "bin/fframes-studio.exe"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"unsigned executable")
            (package / "qualification.json").write_text(json.dumps(packaging.package_record("x86_64-pc-windows-msvc", True, 1.0)))
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
            (package / "qualification.json").write_text(json.dumps(packaging.package_record("x86_64-unknown-linux-gnu", False, 1.0)))
            outside = root / "outside"
            outside.write_text("outside package")
            (package / "linked-file").symlink_to(outside)

            with self.assertRaisesRegex(ValueError, "symlink"):
                finalizing.finalize(package)

    def test_release_workflow_requires_native_signing_for_tag_or_opt_in_builds(self):
        workflow = (assembly.ROOT / ".github/workflows/desktop-release.yml").read_text()
        windows_signing = (assembly.ROOT / "desktop/scripts/sign-windows-package.ps1").read_text()

        for secret in ("WINDOWS_CODESIGN_PFX_BASE64", "WINDOWS_CODESIGN_PFX_PASSWORD"):
            self.assertIn(secret, workflow)
            self.assertIn(secret, windows_signing)

        self.assertIn("inputs.sign_artifacts == true", workflow)
        self.assertIn("github.event_name == 'push'", workflow)
        self.assertIn("verify /pa /all /v", windows_signing)
        self.assertIn("attestations: write", workflow)
        self.assertIn("id-token: write", workflow)
        self.assertIn("actions/attest-build-provenance@v3", workflow)
        self.assertLess(
            workflow.index("Sign Windows executables and FFmpeg DLLs"),
            workflow.index("Attest native app package provenance"),
        )
        # The installer is built from the signed package, with the same certificate still imported.
        self.assertIn("-InstallerPath", workflow)
        self.assertIn("--sign-command", windows_signing)
        self.assertLess(
            windows_signing.index("finalize-native-package.py"),
            windows_signing.index("package-windows-installer.py"),
        )
        self.assertLess(
            workflow.index("Install pinned Inno Setup"),
            workflow.index("Sign Windows executables and FFmpeg DLLs"),
        )
        self.assertLess(
            workflow.index("Sign Windows executables and FFmpeg DLLs"),
            workflow.index("Package unsigned Windows installer candidate"),
        )
        self.assertLess(
            workflow.index("Package unsigned Windows installer candidate"),
            workflow.index("Attest Windows installer candidate provenance"),
        )
        self.assertLess(
            workflow.index("Check Windows signing credentials before building"),
            workflow.index("Focused tests and lint"),
        )
        windows_preflight = workflow.split("Check Windows signing credentials before building", 1)[1].split(
            "Assert native architecture", 1
        )[0]
        self.assertIn("WINDOWS_CODESIGN_PFX_BASE64", windows_preflight)
        mac_packager = (assembly.ROOT / "desktop/scripts/package-phase-zero.py").read_text()
        self.assertIn('"CFBundleInfoDictionaryVersion": "6.0"', mac_packager)
        # The bundle version follows the app crate, so a release bump needs no second edit.
        self.assertIn('"CFBundleVersion": app_version', mac_packager)
        self.assertIn('(ROOT / "desktop/app/Cargo.toml")', mac_packager)


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

    @unittest.skipUnless(os.name == "nt", "reads Windows PE import tables")
    def test_windows_package_refuses_visual_cpp_redistributable_imports(self):
        # CPython links the C runtime dynamically, so its executable imports VCRUNTIME140.dll.
        self.assertTrue(any(name.lower().startswith("vcruntime") for name in packaging.pe_imports(sys.executable)))
        with tempfile.TemporaryDirectory() as temp:
            shutil.copy2(sys.executable, Path(temp) / "python.exe")
            with self.assertRaisesRegex(ValueError, r"python\.exe needs the Visual C\+\+ Redistributable"):
                packaging.windows_dependencies(temp)
        with tempfile.TemporaryDirectory() as temp:
            shutil.copy2(Path(os.environ["SystemRoot"]) / "System32" / "kernel32.dll", temp)
            self.assertTrue(packaging.windows_dependencies(temp).startswith("# kernel32.dll\n"))

    def test_python_lookup_mirrors_the_rust_sibling_lookup(self):
        source = (assembly.ROOT / "desktop/crates/studio-engine/src/agent_tools.rs").read_text()
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


if __name__ == "__main__":
    unittest.main()
