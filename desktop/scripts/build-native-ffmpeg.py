#!/usr/bin/env python3
"""Build the pinned native static FFmpeg install used by the SDK assembler."""
import argparse
import os
from pathlib import Path
import subprocess

COMMIT = "d32b387f2b0a484599d4587d651891f0c63c4238"  # Official n9.0 release commit.


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
    subprocess.run([str(source.resolve() / "configure"), "--prefix=" + str(output.resolve()), "--disable-autodetect", "--disable-programs", "--disable-doc", "--enable-static", "--disable-shared", "--enable-pic"], cwd=source, check=True)
    subprocess.run(["make", "-j" + str(min(os.cpu_count() or 2, 4))], cwd=source, check=True)
    subprocess.run(["make", "install"], cwd=source, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    build(args.source.resolve(), args.out.resolve())
