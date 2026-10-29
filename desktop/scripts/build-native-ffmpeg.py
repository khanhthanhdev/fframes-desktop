#!/usr/bin/env python3
"""Build the pinned native static FFmpeg install used by the SDK assembler."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

COMMIT = "d32b387f2b0a484599d4587d651891f0c63c4238"  # Official n9.0 release commit.
# H.264 encoder (BSD-2-Clause); FFmpeg n9.0 needs openh264 >= 1.3.0.
OPENH264_TAG = "v2.7.0"
OPENH264_SHA256 = "5178775c8b440073e70920e97d195efff721c0e56d0a78cc82c919e95d54924d"
OPENH264_URL = f"https://github.com/cisco/openh264/archive/refs/tags/{OPENH264_TAG}.tar.gz"


def build_openh264(work, prefix):
    """Download, verify and build the pinned OpenH264 as a static library into `prefix`."""
    if platform.machine() in ("x86_64", "AMD64") and shutil.which("nasm") is None:
        raise ValueError("OpenH264 needs nasm for its x86_64 assembly; install nasm")
    archive = work / "openh264.tar.gz"
    with urllib.request.urlopen(OPENH264_URL, timeout=600) as response, archive.open("wb") as file:
        shutil.copyfileobj(response, file)
    with archive.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != OPENH264_SHA256:
            raise ValueError("OpenH264 archive does not match its pinned SHA-256")
    with tarfile.open(archive) as tar:
        tar.extractall(work, filter="data")
    source = work / ("openh264-" + OPENH264_TAG.removeprefix("v"))
    make = ["make", "-j" + str(min(os.cpu_count() or 2, 4)), "PREFIX=" + str(prefix)]
    if sys.platform == "darwin":
        make.append("STATIC_LDFLAGS=-lc++")  # The default -lstdc++ does not exist on macOS.
    subprocess.run(make + ["install-static"], cwd=source, check=True)


def merge_openh264(libavcodec, openh264):
    """Add OpenH264's objects to libavcodec.a: projects link the SDK FFmpeg through
    FFMPEG_DIR, which links only FFmpeg's own libraries. The C++ runtime OpenH264 needs
    comes from Skia's link."""
    merged = libavcodec.with_name("libavcodec-merged.a")
    if sys.platform == "darwin":
        subprocess.run(["xcrun", "libtool", "-static", "-o", str(merged), str(libavcodec), str(openh264)], check=True)
    else:
        script = f"CREATE {merged}\nADDLIB {libavcodec}\nADDLIB {openh264}\nSAVE\nEND\n"
        subprocess.run(["ar", "-M"], input=script, text=True, check=True)
        subprocess.run(["ranlib", str(merged)], check=True)
    symbols = subprocess.check_output(["nm", "-g", str(merged)], text=True, stderr=subprocess.DEVNULL)
    if "WelsCreateSVCEncoder" not in symbols:
        raise ValueError("Merged libavcodec.a lacks OpenH264")
    merged.replace(libavcodec)


def build(source, output):
    if os.name == "nt":
        raise ValueError("Windows uses the verified FFmpeg 9 shared install via FFMPEG_DIR")
    if output.exists():
        raise ValueError("FFmpeg output exists; choose a fresh directory")
    if not source.exists():
        subprocess.run(["git", "clone", "--depth", "1", "--branch", "n9.0", "https://github.com/FFmpeg/FFmpeg.git", str(source)], check=True)
    revision = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if revision != COMMIT:
        raise ValueError("FFmpeg source does not match the pinned release commit")
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        openh264 = work / "openh264-prefix"
        build_openh264(work, openh264)
        environment = dict(os.environ, PKG_CONFIG_PATH=os.pathsep.join(filter(None, [str(openh264 / "lib/pkgconfig"), os.environ.get("PKG_CONFIG_PATH")])))
        subprocess.run([str(source.resolve() / "configure"), "--prefix=" + str(output.resolve()), "--disable-autodetect", "--disable-programs", "--disable-doc", "--enable-static", "--disable-shared", "--enable-pic", "--enable-libopenh264"], cwd=source, env=environment, check=True)
        subprocess.run(["make", "-j" + str(min(os.cpu_count() or 2, 4))], cwd=source, check=True)
        subprocess.run(["make", "install"], cwd=source, check=True)
        merge_openh264(output.resolve() / "lib/libavcodec.a", openh264 / "lib/libopenh264.a")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    build(args.source.resolve(), args.out.resolve())
