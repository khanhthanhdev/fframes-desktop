#!/usr/bin/env python3
"""Assemble a native SDK from an actual Rust sysroot and FFmpeg install."""
import argparse
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
import tomllib

ROOT = Path(__file__).resolve().parents[2]
CRATES = ["fframes", "fframes-media", "svgr-macro", "media-dir-macro", "webvtt-parser", "fframes-studio-protocol", "fframes-studio-runtime"]


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
    with tarfile.open(path, "w:gz", dereference=True) as tar:
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
    for library in ["libavcodec", "libavformat", "libavutil", "libswscale", "libswresample"]:
        if not (prefix / "include" / library).is_dir():
            raise ValueError(f"Missing FFmpeg headers: {library}")
        stem = library.removeprefix("lib")
        if not any((prefix / "lib").glob(f"*{stem}*")):
            raise ValueError(f"Missing FFmpeg library: {library}")
    return version


def configure_bundled_ffmpeg(framework):
    # The build feature downloads/builds FFmpeg before considering FFMPEG_DIR.
    # SDK projects must link the verified install shipped beside the framework.
    media_manifest = framework / "fframes-media/Cargo.toml"
    source = media_manifest.read_text()
    original = 'features = ["build", "static"]'
    if source.count(original) != 1:
        raise ValueError("Unexpected native FFmpeg dependency; review bundled link configuration")
    media_manifest.write_text(source.replace(original, 'features = ["static"]'))


def verify_preview_worker(binary, worker, environment):
    """Exercise the actual offline-built entry before declaring preview support."""
    identity = dict(project_id="sdk-probe", open_session="assembly", source_revision="sdk-probe-revision", worker_generation=7)
    with socket.socket() as listener, tempfile.TemporaryDirectory(prefix="preview-probe-") as cache, tempfile.TemporaryFile() as errors:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(120)
        process = subprocess.Popen([str(binary), "--worker", "--preview-worker", "--frame-port", str(listener.getsockname()[1]),
                                    "--project-id", identity["project_id"], "--open-session", identity["open_session"],
                                    "--revision", identity["source_revision"], "--generation", "7", "--audio-cache", cache],
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
                if hello["identity"] != identity or hello["contract_version"] != 1 or hello["backend"] != "cpu":
                    raise ValueError("Preview hello did not bind launch identity/backend")
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
        for subdir in ["include", "lib", "bin"]:
            if (ffmpeg_root / subdir).is_dir():
                shutil.copytree(ffmpeg_root / subdir, staging / "ffmpeg" / subdir, symlinks=False)
        framework = staging / "framework/framework"
        framework.mkdir(parents=True)
        for crate in CRATES:
            shutil.copytree(ROOT / crate, framework / crate, ignore=shutil.ignore_patterns("target", ".git"))
        configure_bundled_ffmpeg(framework)
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        dependencies = {k: v for k, v in workspace["dependencies"].items() if not isinstance(v, dict) or "path" not in v or v["path"] in CRATES}
        content = '[workspace]\nresolver = "2"\nmembers = ' + toml_value(CRATES) + "\ndependencies = " + toml_value(dependencies) + "\nlints = " + toml_value(workspace["lints"]) + "\n"
        (framework / "Cargo.toml").write_text(content)
        worker = staging / "worker"
        shutil.copytree(ROOT / "desktop/fixtures/annotated-video-overlay", worker, ignore=shutil.ignore_patterns("target"))
        (worker / "Cargo.toml").write_text('[package]\nname="sdk-qualification-worker"\nversion="0.1.0"\nedition="2024"\n[workspace]\n[dependencies]\nfframes={path="../framework/framework/fframes",features=["cli","compile-time-svgtree"]}\nfframes-studio-runtime={path="../framework/framework/fframes-studio-runtime"}\nfframes-studio-protocol={path="../framework/framework/fframes-studio-protocol"}\nsha2="0.10"\n')
        vendor_dir = staging / "framework/vendor"
        if vendor:
            shutil.copytree(vendor, vendor_dir)
        else:
            run(["cargo", "vendor", "--versioned-dirs", "--manifest-path", str(worker / "Cargo.toml"), str(vendor_dir)], stdout=subprocess.DEVNULL)
        # Prove the archive inputs can compile without ambient Cargo downloads.
        (worker / ".cargo").mkdir(exist_ok=True)
        (worker / ".cargo/config.toml").write_text('[source.crates-io]\nreplace-with="vendored-sources"\n[source.vendored-sources]\ndirectory=' + json.dumps(str(vendor_dir)) + '\n')
        environment = {k: v for k, v in os.environ.items() if not k.startswith(("CARGO_", "RUST"))}
        environment.update(PATH=str(staging / "toolchain/bin") + os.pathsep + environment.get("PATH", ""),
                           CARGO_HOME=str(staging / "cargo-home"), CARGO_NET_OFFLINE="true",
                           CARGO_TARGET_DIR=str(staging / "build-output"), FFMPEG_DIR=str(staging / "ffmpeg"),
                           RUSTC=str(staging / "toolchain/bin" / ("rustc.exe" if os.name == "nt" else "rustc")))
        if os.name == "nt":
            environment["PATH"] = str(staging / "ffmpeg/bin") + os.pathsep + environment["PATH"]
        cargo = str(staging / "toolchain/bin" / ("cargo.exe" if os.name == "nt" else "cargo"))
        for attempt in ["first", "second"]:
            environment["CARGO_TARGET_DIR"] = str(staging / f"build-{attempt}")
            run([cargo, "build", "--offline"], cwd=worker, env=environment)
            run([cargo, "run", "--offline", "--", "frame", "0", "-o", "frames"], cwd=worker, env=environment)
            png = worker / "frames/0.png"
            if not png.is_file() or png.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
                raise ValueError("Candidate SDK did not produce a valid frame PNG")
            binary = Path(environment["CARGO_TARGET_DIR"]) / "debug" / ("sdk-qualification-worker.exe" if os.name == "nt" else "sdk-qualification-worker")
            verify_preview_worker(binary, worker, environment)
        artifacts = [archive(staging / name, output, f"{name}-{target}", name) for name in ["toolchain", "ffmpeg", "framework"]]
    manifest.update(sdk_id=f"studio-sdk-{target}-v2", target_triple=target, arch=target.split("-")[0], artifacts=artifacts,
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
    (output / "provenance.json").write_text(json.dumps({"rustc": version, "target": target, "ffmpeg_observed_version": observed_ffmpeg, "offline_worker_builds": 2, "frame_png_verified": True, "preview_contract_verified": True, "qualification": "PENDING: sterile clean-account native run required; local build evidence is not that gate"}, indent=2) + "\n")
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
