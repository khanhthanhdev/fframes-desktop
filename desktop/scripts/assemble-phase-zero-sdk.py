#!/usr/bin/env python3
"""Assemble a native SDK from an actual Rust sysroot and FFmpeg install."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
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
    valid = {expected, expected.removesuffix(".0")}
    if not any(re.match(rf"^n?{re.escape(v)}(?:$|[-+])", version) for v in valid):
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
        artifacts = [archive(staging / name, output, f"{name}-{target}", name) for name in ["toolchain", "ffmpeg", "framework"]]
    manifest.update(sdk_id=f"studio-sdk-{target}-v1", target_triple=target, arch=target.split("-")[0], artifacts=artifacts)
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
    (output / "provenance.json").write_text(json.dumps({"rustc": version, "target": target, "ffmpeg_observed_version": observed_ffmpeg, "offline_worker_builds": 2, "frame_png_verified": True, "qualification": "PENDING: sterile clean-account native run required; local build evidence is not that gate"}, indent=2) + "\n")
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
        candidates = sorted(Path(metadata["target_directory"]).glob("*/build/ffmpeg-sys-fframes-*/out/dist"))
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
