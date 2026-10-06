#!/usr/bin/env python3
"""Preset snapshot reproducibility with the managed SDK.

Generates a fresh project, renders the *same* `src/lib.rs` under two bundled presets
(the pixels must differ), takes a project override and a Reapply, closes and reopens the
project and finally relocates it, then compares the token / font / resource hashes and the
rendered pixels of every stage. A legacy project without a style snapshot must also open,
build and render, and only gain `style/` once a preset is applied.

The Rust half is `desktop/app/tests/real_sdk_presets.rs` (an ignored test); this script
prepares the SDK, runs it and then re-checks everything it wrote *independently*: file
hashes are recomputed here from the copies the test saved, and the project's font is
compared with the bundled preset's own font bytes.

SDK lookup: `--sdk`, else `SDK_ACTIVE`, else `~/.fframes/sdk/active`. If the selected
SDK's `fframes` has no `styles` feature, the script builds an *overlay* SDK (symlinks to
the managed toolchain, FFmpeg and vendored crates, plus a copy of the framework whose
`fframes` is this checkout's) and reports that choice. An SDK with `styles` is used
directly, without an overlay. It never renders without a real compiler or fabricates a
result: a missing SDK exits 2 naming exactly what is missing.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BUILTINS = ROOT / "desktop/crates/studio-presets/builtins"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def tree_hashes(root, prefix):
    """`path relative to root -> sha256` of every file under `root/prefix` (empty if absent)."""
    base = root / prefix
    if not base.is_dir():
        return {}
    return {
        path.relative_to(root).as_posix(): digest(path)
        for path in sorted(base.rglob("*"))
        if path.is_file()
    }


def find_sdk(explicit):
    candidates = [explicit] if explicit else []
    if os.environ.get("SDK_ACTIVE"):
        candidates.append(Path(os.environ["SDK_ACTIVE"]))
    candidates.append(Path.home() / ".fframes/sdk/active")
    sdk = next((c for c in candidates if c and c.exists()), None)
    if sdk is None:
        raise SystemExit(
            "MISSING: no managed SDK found. Looked at: "
            + ", ".join(str(c) for c in candidates if c)
            + ". Install the managed SDK (Setup > Install managed SDK) or set SDK_ACTIVE."
        )
    sdk = sdk.resolve()
    required = [
        "compatibility.json",
        "toolchain/bin/cargo",
        "framework/framework/fframes/Cargo.toml",
        "framework/vendor",
        "ffmpeg",
    ]
    missing = [name for name in required if not (sdk / name).exists()]
    if missing:
        raise SystemExit(f"MISSING: the SDK at {sdk} lacks {', '.join(missing)}")
    return sdk


def has_styles(sdk):
    manifest = (sdk / "framework/framework/fframes/Cargo.toml").read_text()
    return any(line.strip().startswith("styles") and "=" in line for line in manifest.splitlines()) and (
        sdk / "framework/framework/fframes/src/styles.rs"
    ).is_file()


def overlay_sdk(sdk, destination):
    """The managed SDK with this checkout's `fframes` (which has the `styles` feature)."""
    repo = ROOT / "fframes"
    needed = [repo / "Cargo.toml", repo / "src/lib.rs", repo / "src/styles.rs"]
    missing = [str(p) for p in needed if not p.is_file()]
    if missing:
        raise SystemExit(f"MISSING: the checkout has no fframes `styles` sources: {', '.join(missing)}")
    destination.mkdir(parents=True)
    for name in ["toolchain", "ffmpeg", "cargo"]:
        if (sdk / name).exists():
            (destination / name).symlink_to(sdk / name)
    shutil.copy2(sdk / "compatibility.json", destination / "compatibility.json")
    framework = destination / "framework"
    framework.mkdir()
    (framework / "vendor").symlink_to(sdk / "framework/vendor")
    shutil.copytree(sdk / "framework/framework", framework / "framework", symlinks=True)
    target = framework / "framework/fframes"
    shutil.copy2(repo / "Cargo.toml", target / "Cargo.toml")
    shutil.copy2(repo / "src/lib.rs", target / "src/lib.rs")
    shutil.copy2(repo / "src/styles.rs", target / "src/styles.rs")
    return destination


def run_rust(sdk, out):
    environment = dict(os.environ, SDK_ACTIVE=str(sdk), PRESET_RELOCATION_OUT=str(out))
    command = [
        "cargo", "test", "--locked", "--manifest-path", "desktop/Cargo.toml",
        "-p", "fframes-studio", "--test", "real_sdk_presets",
        "--", "--ignored", "--nocapture",
    ]
    print("+", " ".join(command), flush=True)
    result = subprocess.run(command, cwd=ROOT, env=environment)
    if result.returncode != 0:
        raise SystemExit(f"FAILED: the real-SDK preset test exited {result.returncode}")


def check(condition, message):
    if not condition:
        raise SystemExit(f"FAILED: {message}")


def verify(out):
    report = json.loads((out / "report.json").read_text())
    stages = {s["name"]: s for s in report["stages"]}
    names = ["editorial", "pulse", "overridden", "reopened", "relocated"]
    check(list(stages) == names, f"unexpected stages {list(stages)}")
    # Recompute every hash here from the saved copies and the PNG files.
    for name, stage in stages.items():
        base = out / "stages" / name
        style = tree_hashes(base, "style")
        resources = {
            f"media/{path.name}": digest(path)
            for path in sorted((base / "media").glob("preset-*"))
            if path.is_file()
        }
        check(style == stage["tokens"], f"{name}: style hashes differ from the test's record")
        check(resources == stage["resources"], f"{name}: preset resource hashes differ")
        check(digest(out / stage["png"]) == stage["png_sha256"], f"{name}: PNG hash differs")
        identity = json.loads((base / "style/preset.json").read_text())
        check(
            digest(base / "style/tokens.json") == identity["tokens_hash"],
            f"{name}: style/tokens.json is not the snapshot its identity names",
        )
        manifest = json.loads((base / "studio.json").read_text())
        check(
            manifest["preset"] == {"id": identity["id"], "sha256": identity["hash"]},
            f"{name}: studio.json does not reference the applied preset",
        )
    # Identical source under two presets, different output.
    check(len({s["lib_rs"] for s in stages.values()}) == 1, "src/lib.rs changed between stages")
    check(stages["editorial"]["preset"] == "editorial" and stages["pulse"]["preset"] == "pulse", "presets")
    check(
        stages["editorial"]["pixel_sha256"] != stages["pulse"]["pixel_sha256"],
        "two presets rendered identical pixels",
    )
    check(
        stages["pulse"]["pixel_sha256"] != stages["overridden"]["pixel_sha256"],
        "the project override did not change the render",
    )
    # The font in the project is the bundled preset's own font, byte for byte.
    fonts = {k: v for k, v in stages["pulse"]["resources"].items() if k.endswith(".ttf")}
    check(fonts, "the project snapshot holds no font")
    for relative, hash_ in fonts.items():
        # Flat project name -> package path: `media/preset-fonts-X.ttf` is `fonts/X.ttf`.
        bundled = BUILTINS / "pulse" / relative.removeprefix("media/preset-").replace("fonts-", "fonts/", 1)
        check(bundled.is_file() and digest(bundled) == hash_, f"{relative} is not the bundled font")
    # Overrides survive reopen, and everything is stable across relocation.
    reference = stages["overridden"]
    for name in ["reopened", "relocated"]:
        stage = stages[name]
        check(stage["overrides"] == reference["overrides"] == 1, f"{name}: override count")
        for key in ["tokens", "resources", "pixel_sha256", "preset_hash"]:
            check(stage[key] == reference[key], f"{name}: {key} differs from before")
    check(stages["relocated"]["root"] != stages["overridden"]["root"], "the project did not move")
    # The generated project's own `inspect` (exit 2 on a missing font; the renderer loads no
    # system fonts) passes with the preset's top-level font file and FAILS once that file is
    # removed, so the font really comes from the bundled file, for each font file in turn.
    inspections = report["inspections"]
    for label in ["editorial", "pulse", "quiet_motion"]:
        ok = inspections[f"{label}_with_font"]
        check(ok["exit"] == 0, f"inspect with the {label} font exited {ok['exit']}: {ok['output']}")
    for label in ["editorial", "quiet_motion"]:
        gone = inspections[f"{label}_font_removed"]
        check(gone["exit"] == 2, f"inspect without the {label} font exited {gone['exit']}, not 2: {gone['output']}")
        check("font" in gone["output"].lower(), f"{label}: the failure does not name a font: {gone['output']}")
    legacy = report["legacy"]
    check(
        legacy["opened_without_styles"] and legacy["lib_rs_unchanged"] and legacy["cargo_unchanged"],
        "legacy project",
    )
    check(legacy["template"] == "pre-m4", "legacy project is not the pre-M4 scaffold")
    legacy_fixture = ROOT / "desktop/app/tests/fixtures/pre_m4_project"
    check(
        digest(legacy_fixture / "src/lib.rs") == legacy["lib_rs_sha256"],
        "legacy lib.rs differs from the checked-in pre-M4 scaffold",
    )
    check(
        digest(legacy_fixture / "Cargo.toml") == legacy["cargo_sha256"],
        "legacy Cargo.toml differs from the checked-in pre-M4 scaffold",
    )
    check(legacy["pixel_sha256"], "legacy render")
    # Report.
    print("\nPreset relocation: PASS")
    print(f"  SDK id: {report['sdk_id']}")
    print(f"  editorial pixels : {stages['editorial']['pixel_sha256'][:16]}")
    print(f"  pulse pixels     : {stages['pulse']['pixel_sha256'][:16]} (differs from editorial)")
    print(f"  overridden pixels: {reference['pixel_sha256'][:16]}")
    for name in ["reopened", "relocated"]:
        print(f"  {name:<9} pixels : {stages[name]['pixel_sha256'][:16]} (matches overridden)")
    for label, key in [("token", "tokens"), ("font/resource", "resources")]:
        files = reference[key]
        print(f"  {label} hashes match before and after relocation: {len(files)} files")
        for path, hash_ in files.items():
            print(f"    {hash_[:16]}  {path}")
    print("  inspect (no system fonts): passes with each preset's top-level font file, exits 2 when it is removed")
    for label, result in inspections.items():
        print(f"    {label:<28} exit {result['exit']}")
    print(
        "  pre-M4 legacy project without styles: opened, built and rendered "
        f"({legacy['pixel_sha256'][:16]}); applying a preset later left src/lib.rs and Cargo.toml untouched"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sdk", type=Path, help="installed SDK directory")
    parser.add_argument("--out", type=Path, help="fresh evidence directory (default: a temporary one)")
    args = parser.parse_args()
    managed = find_sdk(args.sdk)
    work = Path(tempfile.mkdtemp(prefix="preset-relocation-")) if args.out is None else args.out.resolve()
    if args.out is not None:
        if work.exists():
            raise SystemExit("Evidence directory exists; choose a fresh directory")
        work.mkdir(parents=True)
    try:
        if has_styles(managed):
            sdk = managed
            print(f"SDK {managed} already provides fframes `styles`")
        else:
            sdk = overlay_sdk(managed, work / "sdk")
            print(
                f"NOTE: the managed SDK at {managed} predates `fframes::Styles` (its fframes has no "
                f"`styles` feature); using an overlay SDK at {sdk} with this checkout's fframes. A new "
                "SDK release must be assembled before shipping."
            )
        out = work / "evidence"
        out.mkdir()
        run_rust(sdk, out)
        verify(out)
        if args.out is not None:
            print(f"  evidence kept in {work}")
    finally:
        if args.out is None:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
