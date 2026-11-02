#!/usr/bin/env python3
"""Assemble a native SDK from an actual Rust sysroot and FFmpeg install."""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import tarfile
import tempfile
import threading
import urllib.request
import tomllib

ROOT = Path(__file__).resolve().parents[2]
# fframes-three-d backs fframes' `3d` feature, which fframes-kit's `3d` feature turns on.
CRATES = ["fframes", "fframes-media", "svgr-macro", "media-dir-macro", "webvtt-parser", "fframes-three-d", "fframes-studio-protocol", "fframes-studio-runtime", "fframes-studio-kit", "fframes-skia-renderer"]
# fframes-kit features the qualification worker enables, so the vendor folder holds every
# optional dependency a project may turn on. `skia` is left out: it only marks code for the
# Skia backend and adds no dependency. A kit release with other features fails assembly.
KIT_FEATURES = ["3d", "biology", "chemistry", "cs", "gif", "latex", "lottie", "math", "ml", "physics", "studio"]
# The qualification frame of the kit, copied into the worker as an example.
KIT_FRAME = "desktop/scripts/sdk-kit-frame.rs"
# This bundle's release. Projects pin it; those of earlier releases are not built by it.
SDK_RELEASE = "v5"
OVERLAY = "desktop/fixtures/annotated-video-overlay"
# The reviewed prebuilt Skia archive of each target, see `stage_skia`.
SKIA_PINS = "desktop/packaging/sdk/skia-binaries.json"
# Host variables that would make skia-bindings download, build or bind another Skia.
SKIA_VARIABLE_PREFIXES = ("SKIA_", "FORCE_SKIA_")
# The FFmpeg libraries fframes links; a built worker loads no other FFmpeg DLL.
FFMPEG_LIBRARIES = ["libavcodec", "libavformat", "libavutil", "libswscale", "libswresample"]


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def toml_value(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{json.dumps(k)} = {toml_value(v)}" for k, v in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(v) for v in value) + "]"
    return json.dumps(value)


def archive(source, output, name, destination):
    path = output / "artifacts" / f"{name}.tar.gz"
    # Level 6 instead of tarfile's 9: several times faster for a few percent more bytes.
    with tarfile.open(path, "w:gz", compresslevel=6, dereference=True) as tar:
        for entry in sorted(source.iterdir()):
            tar.add(entry, arcname=entry.name)
    with path.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"name": destination, "url": f"file://artifacts/{path.name}", "sha256": checksum, "size_bytes": path.stat().st_size, "destination_subdir": destination}


def ffmpeg_version(prefix):
    version_file = prefix / "include/libavutil/ffversion.h"
    if version_file.is_file():
        match = re.search(r'#define FFMPEG_VERSION "([^"\n]+)"', version_file.read_text())
        if match:
            return match[1]
    binary = prefix / "bin" / ("ffmpeg.exe" if os.name == "nt" else "ffmpeg")
    if binary.is_file():
        output = run([str(binary), "-version"], capture_output=True).stdout
        match = re.search(r"ffmpeg version (\S+)", output)
        if match:
            return match[1]
    raise ValueError("FFmpeg install has no verifiable version header or executable")


def validate_ffmpeg(prefix, expected):
    version = ffmpeg_version(prefix)
    major_minor = (
        re.match(r"^(\d+\.\d+)", expected).group(1)
        if re.match(r"^(\d+\.\d+)", expected)
        else expected
    )
    pattern = rf"^n?{re.escape(major_minor)}(?:\.\d+)?(?:$|[-+._])"
    if not re.match(pattern, version):
        raise ValueError(f"Expected FFmpeg {expected}, found {version}; refusing to mislabel host libraries")
    for library in FFMPEG_LIBRARIES:
        if not (prefix / "include" / library).is_dir():
            raise ValueError(f"Missing FFmpeg headers: {library}")
        stem = library.removeprefix("lib")
        if not any((prefix / "lib").glob(f"*{stem}*")):
            raise ValueError(f"Missing FFmpeg library: {library}")
    return version


def prune_toolchain(toolchain, target):
    """Drop what a native build for `target` never uses: other targets' standard libraries
    (the host may have them installed), rust-src, debug symbols and the wasm component linker.
    MSVC targets link with Visual Studio's link.exe, so they also drop rust-lld (108 MB); it
    stays elsewhere, x86_64 Linux links with it by default."""
    rustlib = toolchain / "lib/rustlib"
    for entry in rustlib.iterdir():
        if entry.is_dir() and entry.name not in {target, "etc"}:
            shutil.rmtree(entry)
    if target.endswith("windows-msvc"):
        (rustlib / target / "bin/rust-lld.exe").unlink()
        shutil.rmtree(rustlib / target / "bin/gcc-ld")
    for path in toolchain.rglob("*"):
        if path.is_file() and (path.suffix == ".pdb" or path.stem == "wasm-component-ld"):
            path.unlink()


def copy_ffmpeg(prefix, destination, link_mode):
    """Copy headers and link libraries; for a shared build only the DLLs of FFMPEG_LIBRARIES.
    Static builds need no `bin`, and nothing runs the ffmpeg, ffprobe or ffplay programs."""
    for subdir in ["include", "lib"]:
        shutil.copytree(prefix / subdir, destination / subdir, symlinks=False)
    if link_mode == "shared":
        (destination / "bin").mkdir()
        for library in FFMPEG_LIBRARIES:
            for dll in (prefix / "bin").glob(f"{library.removeprefix('lib')}-*.dll"):
                shutil.copy(dll, destination / "bin" / dll.name)


def prune_vendor(vendor_dir, used):
    """Delete the prebuilt import libraries (`lib/`) of vendored crates that no build for this
    target compiles, such as winapi's and windows-targets' crates for other targets. Cargo
    still resolves the full lockfile, so the crates and their sources stay; their checksum
    lists drop the deleted files, which Cargo then does not look for."""
    for crate in vendor_dir.iterdir():
        if crate.name in used or not (crate / "lib").is_dir():
            continue
        shutil.rmtree(crate / "lib")
        checksums_path = crate / ".cargo-checksum.json"
        checksums = json.loads(checksums_path.read_text())
        checksums["files"] = {k: v for k, v in checksums["files"].items() if not k.startswith("lib/")}
        checksums_path.write_text(json.dumps(checksums))


def target_packages(cargo, worker, environment, target):
    """The `name-version` vendor folders of the packages a build of `worker` for `target` uses."""
    metadata = json.loads(run([cargo, "metadata", "--offline", "--format-version", "1", "--filter-platform", target],
                              cwd=worker, env=environment, capture_output=True, encoding="utf-8").stdout)
    packages = {p["id"]: f"{p['name']}-{p['version']}" for p in metadata["packages"]}
    return {packages[node["id"]] for node in metadata["resolve"]["nodes"]}


def configure_bundled_ffmpeg(framework):
    # The build feature downloads/builds FFmpeg before considering FFMPEG_DIR.
    # SDK projects must link the verified install shipped beside the framework.
    media_manifest = framework / "fframes-media/Cargo.toml"
    source = media_manifest.read_text()
    original = 'features = ["build", "static"]'
    if source.count(original) != 1:
        raise ValueError("Unexpected native FFmpeg dependency; review bundled link configuration")
    media_manifest.write_text(source.replace(original, 'features = ["static"]'))


def configure_bundled_skia(framework):
    # SDK projects link the pinned archive shipped in the framework. `no-compile` turns a key
    # miss into an immediate error instead of a Skia source build that downloads Skia. The
    # dev-dependencies are workspace examples the SDK does not ship.
    manifest = framework / "fframes-skia-renderer/Cargo.toml"
    source = manifest.read_text()
    original = 'skia-safe = { version = "0.153.3", features = ["svg"] }'
    if source.count(original) != 1 or source.count("[dev-dependencies]") != 1:
        raise ValueError("Unexpected Skia dependency; review the bundled Skia configuration")
    source = source.replace(original, 'skia-safe = { version = "0.153.3", features = ["svg", "no-compile"] }')
    source = re.sub(r"\[dev-dependencies\]\n(?:[^\[\n][^\n]*\n|\n)*", "", source)
    manifest.write_text(source)
    for unshipped in ["examples", "tests"]:
        shutil.rmtree(framework / "fframes-skia-renderer" / unshipped, ignore_errors=True)


def stage_skia(target, destination):
    """Copy the reviewed Skia archive of `target` to `destination` and return its pin.

    `SKIA_ARCHIVE_DIR` may hold archives fetched before; otherwise the archive is downloaded
    from the pinned release. Either way it must match the reviewed size and sha256."""
    pins = json.loads((ROOT / SKIA_PINS).read_text())
    pin = pins["archives"].get(target)
    if pin is None:
        raise ValueError(f"No reviewed Skia archive for {target} in {SKIA_PINS}")
    name = f"skia-binaries-{pin['key']}.tar.gz"
    destination.mkdir(parents=True)
    archive_path = destination / name
    cached = Path(os.environ["SKIA_ARCHIVE_DIR"]) / name if os.environ.get("SKIA_ARCHIVE_DIR") else None
    if cached and cached.is_file():
        shutil.copy(cached, archive_path)
    else:
        with urllib.request.urlopen(f"{pins['release_url']}/{name}", timeout=600) as response, archive_path.open("wb") as file:
            shutil.copyfileobj(response, file)
    with archive_path.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    if checksum != pin["sha256"] or archive_path.stat().st_size != pin["size_bytes"]:
        raise ValueError(f"{name} does not match its reviewed pin in {SKIA_PINS}")
    return dict(pin, archive=name)


def skia_binaries_url(skia_dir):
    """The `SKIA_BINARIES_URL` template for archives in `skia_dir` (`{key}` stays literal)."""
    return "file://" + str(skia_dir / "skia-binaries-{key}.tar.gz")


def check_skia_from_archive(target_dir, skia):
    """Fail unless skia-bindings installed the shipped archive instead of building or fetching."""
    outputs = list(Path(target_dir).glob("debug/build/skia-bindings-*/output"))
    installed = [o for o in outputs if "FROM: file://" in o.read_text(errors="replace") and skia["key"] in o.read_text(errors="replace")]
    if not installed or any("DOWNLOAD AND INSTALL SUCCEEDED" not in o.read_text(errors="replace") for o in installed):
        raise ValueError("The qualification build did not install the shipped Skia archive")


def write_framework(framework, workspace_toml):
    """Make the copied CRATES in `framework` one workspace with the source's dependencies."""
    configure_bundled_ffmpeg(framework)
    configure_bundled_skia(framework)
    workspace = tomllib.loads(workspace_toml)["workspace"]
    dependencies = {}
    for name, dependency in workspace["dependencies"].items():
        if isinstance(dependency, dict) and "path" in dependency and dependency["path"] not in CRATES:
            # Development paths outside the framework do not ship. Keep their explicit
            # registry version, rather than dropping a dependency inherited by a crate.
            if "version" not in dependency:
                raise ValueError(f"Cannot bundle local workspace dependency {name} without a registry version")
            dependency = {key: value for key, value in dependency.items() if key != "path"}
        dependencies[name] = dependency
    content = '[workspace]\nresolver = "2"\nmembers = ' + toml_value(CRATES) + "\ndependencies = " + toml_value(dependencies) + "\nlints = " + toml_value(workspace["lints"]) + "\n"
    # fframes-kit (crates.io) builds against this fframes, as in the source workspace.
    content += '\n[patch.crates-io]\nfframes = { path = "fframes" }\n'
    (framework / "Cargo.toml").write_text(content)


def crate_version(manifest):
    return tomllib.loads(manifest.read_text())["package"]["version"]


def kit_version():
    """The exact fframes-kit release fframes-studio-kit is built with, which the SDK vendors."""
    requirement = tomllib.loads((ROOT / "fframes-studio-kit/Cargo.toml").read_text())["dependencies"]["fframes-kit"]["version"]
    if not re.fullmatch(r"=\d+\.\d+\.\d+", requirement):
        raise ValueError(f"fframes-studio-kit must pin fframes-kit exactly, found {requirement!r}")
    return requirement.removeprefix("=")


def check_kit_features(vendor_dir, kit):
    """Fail when the vendored kit has features the worker does not enable (their optional
    dependencies would be missing from the SDK) or lost one it does."""
    features = set(tomllib.loads((vendor_dir / f"fframes-kit-{kit}/Cargo.toml").read_text()).get("features", {}))
    if features - {"default", "skia"} != set(KIT_FEATURES):
        raise ValueError(f"fframes-kit {kit} has features {sorted(features)}; update KIT_FEATURES")


def worker_manifest(tree, kit):
    return ('[package]\nname="sdk-qualification-worker"\nversion="0.1.0"\nedition="2024"\n[workspace]\n[dependencies]\n'
            f'fframes={{path="../framework/{tree}/fframes",features=["cli","compile-time-svgtree"]}}\n'
            f'fframes-studio-runtime={{path="../framework/{tree}/fframes-studio-runtime"}}\n'
            f'fframes-studio-protocol={{path="../framework/{tree}/fframes-studio-protocol"}}\nsha2="0.10"\n'
            f'fframes-kit={{version="={kit}",features={toml_value(KIT_FEATURES)}}}\n'
            f'fframes-studio-kit={{path="../framework/{tree}/fframes-studio-kit"}}\n'
            # What build materialization adds to a project that uses the kit.
            f'[patch.crates-io]\nfframes={{path="../framework/{tree}/fframes"}}\n')


def check_backend(hello, renderer):
    """`--renderer skia-cpu` must draw with Skia, which proves it is linked on any host. The
    default draws on the GPU, or with the built-in CPU renderer and its shader gap."""
    backend, gaps = hello["backend"], hello["capability_gaps"]
    if renderer == "skia-cpu":
        valid = backend == "skia-cpu" and "shader_preview" not in gaps
    else:
        valid = (backend == "skia-gpu" and "shader_preview" not in gaps) or (backend == "cpu" and "shader_preview" in gaps)
    if not valid:
        raise ValueError(f"Preview worker reported backend {backend} with gaps {gaps} for renderer {renderer or 'auto'}")


def verify_preview_worker(binary, worker, environment, renderer=None):
    """Exercise the actual offline-built entry before declaring preview support."""
    identity = dict(project_id="sdk-probe", open_session="assembly", source_revision="sdk-probe-revision", worker_generation=7)
    with socket.socket() as listener, tempfile.TemporaryDirectory(prefix="preview-probe-") as cache, tempfile.TemporaryFile() as errors:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(120)
        process = subprocess.Popen([str(binary), "--worker", "--preview-worker", "--frame-port", str(listener.getsockname()[1]),
                                    "--project-id", identity["project_id"], "--open-session", identity["open_session"],
                                    "--revision", identity["source_revision"], "--generation", "7", "--audio-cache", cache]
                                   + (["--renderer", renderer] if renderer else []),
                                   cwd=worker, env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors)
        watchdog = threading.Timer(120, process.kill)
        watchdog.start()
        try:
            connection, _ = listener.accept()
            with connection, connection.makefile("rb") as bulk:
                connection.settimeout(120)
                def read_exact(stream, length):
                    data = bytearray()
                    while len(data) < length:
                        chunk = stream.read(length - len(data))
                        if not chunk:
                            raise ValueError("Preview worker truncated a record")
                        data.extend(chunk)
                    return bytes(data)
                def read_json(stream):
                    length = struct.unpack(">I", read_exact(stream, 4))[0]
                    if length > 1024 * 1024:
                        raise ValueError("Preview worker exceeded control bound")
                    return json.loads(read_exact(stream, length))
                serial = 0
                def request(kind, **fields):
                    nonlocal serial
                    serial += 1
                    envelope = dict(contract_version=1, identity=identity, request_id=serial)
                    body = dict(type=kind, **fields)
                    if kind == "Hello":
                        body["request_id"] = serial
                    elif kind in {"Timeline", "Shutdown"}:
                        body.update(envelope)
                    else:
                        body["envelope"] = envelope
                    encoded = json.dumps(body).encode()
                    process.stdin.write(struct.pack(">I", len(encoded)) + encoded)
                    process.stdin.flush()
                    response = read_json(process.stdout)
                    if response.get("type") == "Error" or (kind != "Hello" and response.get("envelope") != envelope):
                        raise ValueError(f"Invalid preview response: {response}")
                    return response
                hello = request("Hello", offered_versions=[1], required_capabilities=["preview_identity_v1", "scaled_frame_v1", "inspect_v1", "prepared_audio_v1"])
                if hello["identity"] != identity or hello["contract_version"] != 1:
                    raise ValueError("Preview hello did not bind launch identity")
                check_backend(hello, renderer)
                timeline = request("Timeline")
                frame = request("ScaledFrame", frame_index=0, seek_serial=13, scale=0.5)
                record = read_json(bulk)
                if record != frame["record"] or record["payload_len"] > 64 * 1024 * 1024:
                    raise ValueError("Invalid tagged frame record")
                pixels = read_exact(bulk, record["payload_len"])
                if len(pixels) != frame["header"]["payload_len"] or frame["seek_serial"] != 13:
                    raise ValueError("Preview frame geometry/seek mismatch")
                inspected = request("Inspect", frames=[0, timeline["total_frames"] - 1])
                if inspected["truncated"] or any(d["severity"] == "error" for d in inspected["diagnostics"]):
                    raise ValueError("SDK preview inspection failed")
                audio = request("PrepareAudio", output_sample_rate=48000)
                digest = hashlib.sha256()
                for offset in range(0, audio["byte_count"], 256 * 1024):
                    length = min(256 * 1024, audio["byte_count"] - offset)
                    read = request("ReadAudio", artifact_id=audio["artifact_id"], offset=offset, length=length)
                    header = read_json(bulk)
                    if header != read["record"] or header["kind"] != "audio_pcm_f32_le" or header["payload_len"] != length:
                        raise ValueError("Invalid tagged audio record")
                    digest.update(read_exact(bulk, length))
                if digest.hexdigest() != audio["sha256"] or audio["byte_count"] != audio["sample_count"] * 8:
                    raise ValueError("Prepared mix checksum/sample count mismatch")
                request("ReleaseAudio", artifact_id=audio["artifact_id"])
                request("Shutdown")
                process.stdin.close()
                if process.wait(timeout=5) != 0 or any(Path(cache).iterdir()):
                    raise ValueError("Preview worker failed shutdown/artifact cleanup")
        finally:
            watchdog.cancel()
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)


def assemble(output, ffmpeg_root, vendor=None):
    output = output.resolve()
    if output.exists():
        raise ValueError(f"Output already exists; choose a fresh directory: {output}")
    manifest = json.loads((ROOT / "desktop/packaging/sdk/phase-zero-sdk.json").read_text())
    version = run(["rustc", "--version"], capture_output=True).stdout.split()[1]
    if version != manifest["rust_toolchain"]["channel"]:
        raise ValueError(f"Expected Rust {manifest['rust_toolchain']['channel']}, found {version}")
    target = re.search(r"^host: (.+)$", run(["rustc", "-vV"], capture_output=True).stdout, re.M).group(1)
    if target not in {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"}:
        raise ValueError(f"Unsupported native target: {target}")
    observed_ffmpeg = validate_ffmpeg(ffmpeg_root, manifest["ffmpeg"]["tag"])
    link_mode = "shared" if target.endswith("windows-msvc") else "static"
    if link_mode == "static" and not (ffmpeg_root / "lib/libavcodec.a").is_file():
        raise ValueError("Native Unix SDK requires the static FFmpeg install used by fframes")
    output.mkdir(parents=True)
    (output / "artifacts").mkdir()
    with tempfile.TemporaryDirectory(prefix="studio-sdk-") as temporary:
        staging = Path(temporary)
        sysroot = Path(run(["rustc", "--print", "sysroot"], capture_output=True).stdout.strip())
        for subdir in ["bin", "lib"]:
            shutil.copytree(sysroot / subdir, staging / "toolchain" / subdir, symlinks=False)
        prune_toolchain(staging / "toolchain", target)
        copy_ffmpeg(ffmpeg_root, staging / "ffmpeg", link_mode)
        framework = staging / "framework/framework"
        framework.mkdir(parents=True)
        for crate in CRATES:
            shutil.copytree(ROOT / crate, framework / crate, ignore=shutil.ignore_patterns("target", ".git"))
        write_framework(framework, (ROOT / "Cargo.toml").read_text())
        # installed as <sdk>/framework/skia/, where the SDK's SKIA_BINARIES_URL points
        skia_dir = staging / "framework/skia"
        skia = stage_skia(target, skia_dir)
        worker = staging / "worker"
        shutil.copytree(ROOT / OVERLAY, worker, ignore=shutil.ignore_patterns("target"))
        kit = kit_version()
        (worker / "Cargo.toml").write_text(worker_manifest("framework", kit))
        (worker / "examples").mkdir()
        shutil.copy(ROOT / KIT_FRAME, worker / "examples/kit_frame.rs")
        vendor_dir = staging / "framework/vendor"
        if vendor:
            shutil.copytree(vendor, vendor_dir)
        else:
            run(["cargo", "vendor", "--versioned-dirs", "--manifest-path", str(worker / "Cargo.toml"), str(vendor_dir)], stdout=subprocess.DEVNULL)
        check_kit_features(vendor_dir, kit)
        # Prove the archive inputs can compile without ambient Cargo downloads.
        (worker / ".cargo").mkdir(exist_ok=True)
        (worker / ".cargo/config.toml").write_text('[source.crates-io]\nreplace-with="vendored-sources"\n[source.vendored-sources]\ndirectory=' + json.dumps(str(vendor_dir)) + '\n')
        environment = {k: v for k, v in os.environ.items() if not k.startswith(("CARGO_", "RUST", *SKIA_VARIABLE_PREFIXES))}
        environment.update(PATH=str(staging / "toolchain/bin") + os.pathsep + environment.get("PATH", ""),
                           CARGO_HOME=str(staging / "cargo-home"), CARGO_NET_OFFLINE="true",
                           CARGO_TARGET_DIR=str(staging / "build-output"), FFMPEG_DIR=str(staging / "ffmpeg"),
                           RUSTC=str(staging / "toolchain/bin" / ("rustc.exe" if os.name == "nt" else "rustc")),
                           SKIA_BINARIES_URL=skia_binaries_url(skia_dir))
        if os.name == "nt":
            environment["PATH"] = str(staging / "ffmpeg/bin") + os.pathsep + environment["PATH"]
        cargo = str(staging / "toolchain/bin" / ("cargo.exe" if os.name == "nt" else "cargo"))
        # Pruned before the qualification builds below, which then prove the archive builds.
        prune_vendor(vendor_dir, target_packages(cargo, worker, environment, target))
        for directory, attempt in [(worker, "first"), (worker, "second")]:
            environment["CARGO_TARGET_DIR"] = str(staging / f"build-{attempt}")
            run([cargo, "build", "--offline"], cwd=directory, env=environment)
            check_skia_from_archive(environment["CARGO_TARGET_DIR"], skia)
            run([cargo, "run", "--offline", "--", "frame", "0", "-o", "frames"], cwd=directory, env=environment)
            png = directory / "frames/0.png"
            if not png.is_file() or png.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
                raise ValueError("Candidate SDK did not produce a valid frame PNG")
            # A kit component through fframes-studio-kit, from the vendored kit.
            run([cargo, "run", "--offline", "--example", "kit_frame", "--", "frame", "0", "-o", "kit-frames"], cwd=directory, env=environment)
            png = directory / "kit-frames/0.png"
            if not png.is_file() or png.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
                raise ValueError("Candidate SDK did not render the fframes-kit frame")
            binary = Path(environment["CARGO_TARGET_DIR"]) / "debug" / ("sdk-qualification-worker.exe" if os.name == "nt" else "sdk-qualification-worker")
            verify_preview_worker(binary, directory, environment, "skia-cpu")
            verify_preview_worker(binary, directory, environment)
        # zlib releases the GIL while it compresses, so the three archives build in parallel.
        with concurrent.futures.ThreadPoolExecutor() as pool:
            artifacts = list(pool.map(lambda name: archive(staging / name, output, f"{name}-{target}", name), ["toolchain", "ffmpeg", "framework"]))
    if crate_version(ROOT / "fframes-studio-kit/Cargo.toml") != crate_version(ROOT / "fframes-studio-runtime/Cargo.toml"):
        raise ValueError("fframes-studio-kit is released with fframes-studio-runtime; give both the same version")
    manifest.update(sdk_id=f"studio-sdk-{target}-{SDK_RELEASE}", runtime_version=crate_version(ROOT / "fframes-studio-runtime/Cargo.toml"), kit_version=kit, target_triple=target, arch=target.split("-")[0], artifacts=artifacts,
                    preview_contract_versions=[1], preview_capabilities=["preview_identity_v1", "scaled_frame_v1", "inspect_v1", "prepared_audio_v1"])
    manifest["rust_toolchain"]["targets"] = [target]
    manifest["ffmpeg"]["link_mode"] = link_mode
    manifest["ffmpeg"]["bin_rel_path"] = "ffmpeg/bin" if link_mode == "shared" else None
    if target.endswith("windows-msvc"):
        manifest["os_baseline"] = "Windows 11 x64 with Visual Studio C++ Build Tools and Windows SDK"
        manifest["host_prerequisites"] = [{"id": "msvc", "name": "MSVC compiler", "description": "Visual Studio native build environment", "command": "cl.exe", "args": [], "package_name": "Visual Studio C++ Build Tools", "required": True}]
    elif target.endswith("apple-darwin"):
        manifest["os_baseline"] = "macOS arm64 with Xcode Command Line Tools"
        manifest["host_prerequisites"] = [{"id": "xcode", "name": "Xcode Command Line Tools", "description": "Native linker and Apple SDK", "command": "xcrun", "args": ["--find", "clang"], "package_name": "Xcode Command Line Tools", "required": True}]
    (output / "compatibility.json").write_text(json.dumps(manifest, indent=2) + "\n")
    shutil.copytree(ROOT / "desktop/packaging/sdk/notices", output / "notices")
    shutil.copy(ROOT / "desktop/fixtures/annotated-video-overlay/media/OFL.txt", output / "notices/DM-Sans-OFL.txt")
    (output / "provenance.json").write_text(json.dumps({"rustc": version, "target": target, "ffmpeg_observed_version": observed_ffmpeg, "offline_worker_builds": 2, "frame_png_verified": True, "kit_frame_png_verified": True, "skia_archive": skia, "preview_contract_verified": True, "qualification": "PENDING: sterile clean-account native run required; local build evidence is not that gate"}, indent=2) + "\n")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--ffmpeg-root", type=Path, help="Actual FFmpeg install; otherwise use FFMPEG_DIR or a verified native Cargo build install")
    parser.add_argument("--vendor", type=Path, help="Previously vendored worker graph; otherwise Cargo vendors it")
    args = parser.parse_args()
    ffmpeg = args.ffmpeg_root or (Path(os.environ["FFMPEG_DIR"]) if os.environ.get("FFMPEG_DIR") else None)
    if ffmpeg is None:
        metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(ROOT / "desktop/fixtures/annotated-video-overlay/Cargo.toml")], capture_output=True).stdout)
        candidates = sorted(Path(metadata["target_directory"]).glob("**/build/ffmpeg-sys-fframes-*/out/dist"))
        for candidate in candidates:
            try:
                validate_ffmpeg(candidate, "9.0.0")
                ffmpeg = candidate
                break
            except ValueError:
                continue
    if ffmpeg is None:
        raise ValueError("No verified FFmpeg install found; supply --ffmpeg-root")
    assemble(args.out, ffmpeg.resolve(), args.vendor)


if __name__ == "__main__":
    main()
