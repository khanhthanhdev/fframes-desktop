#!/usr/bin/env python3
"""Assemble an unsigned Linux x64 .deb candidate from the native app binaries."""
import argparse
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
BINARIES = ("fframes-studio", "studio-tools", "studio-mcp", "studio_setup")
MAINTAINER_PATTERN = re.compile(r"^.+\s+<[^<>\s@]+@[^<>\s]+>$")


def run(arguments, **kwargs):
    return subprocess.run(arguments, check=True, text=True, **kwargs)


def app_version():
    manifest = tomllib.loads((ROOT / "desktop/app/Cargo.toml").read_text())
    version = manifest["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version):
        raise ValueError(f"App version is not a supported release version: {version}")
    return version.replace("-", "~", 1) if "-" in version else version


def deb_dependencies(stage):
    build_control_dir = stage / "debian"
    build_control_dir.mkdir()
    build_control = build_control_dir / "control"
    package_control = (stage / "DEBIAN/control").read_text()
    maintainer = next(
        line.removeprefix("Maintainer: ")
        for line in package_control.splitlines()
        if line.startswith("Maintainer: ")
    )
    build_control.write_text(
        "Source: fframes-studio\n"
        "Section: video\n"
        "Priority: optional\n"
        f"Maintainer: {maintainer}\n\n"
        f"{package_control}"
    )
    binaries = [stage / "opt/fframes-studio/bin" / name for name in BINARIES]
    try:
        output = run(
            ["dpkg-shlibdeps", "-O", *(f"-e{binary}" for binary in binaries)],
            cwd=stage,
            capture_output=True,
        ).stdout
    finally:
        shutil.rmtree(build_control_dir)
    prefix = "shlibs:Depends="
    dependencies = next(
        (line.removeprefix(prefix) for line in output.splitlines() if line.startswith(prefix)),
        None,
    )
    if not dependencies:
        raise ValueError(f"dpkg-shlibdeps did not produce runtime dependencies for {build_control}")
    return dependencies


def package(source, output, maintainer):
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "amd64"}:
        raise ValueError("Linux .deb candidates are built on Linux x86_64 only")
    for tool in ("dpkg-deb", "dpkg-shlibdeps"):
        if shutil.which(tool) is None:
            raise ValueError(f"Required Debian packaging tool is unavailable: {tool}")
    if not MAINTAINER_PATTERN.fullmatch(maintainer):
        raise ValueError("maintainer must be a name followed by a valid email address")

    source = Path(source).resolve()
    output = Path(output).resolve()
    if not source.is_dir():
        raise ValueError(f"native app package directory does not exist: {source}")
    if output.exists():
        raise ValueError(f"output already exists; choose a fresh path: {output}")
    source_binaries = source / "bin"
    if not source_binaries.is_dir() or source_binaries.is_symlink():
        raise ValueError("native app package has no safe binary directory")
    missing = [name for name in BINARIES if not (source_binaries / name).is_file()]
    if missing:
        raise ValueError(f"native app package is missing binaries: {', '.join(missing)}")
    if any((source_binaries / name).is_symlink() for name in BINARIES):
        raise ValueError("native app binaries must not be symbolic links")
    notices = source / "notices"
    if not notices.is_dir() or notices.is_symlink():
        raise ValueError("native app package is missing third-party notices")
    if any(path.is_symlink() for path in notices.rglob("*")):
        raise ValueError("third-party notice tree must not contain symbolic links")

    version = app_version()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".fframes-studio-deb-", dir=output.parent
    ) as temporary:
        stage = Path(temporary) / "package"
        candidate = Path(temporary) / "candidate.deb"
        debian = stage / "DEBIAN"
        installed = stage / "opt/fframes-studio"
        binary_directory = installed / "bin"
        (stage / "usr/bin").mkdir(parents=True)
        (stage / "usr/share/applications").mkdir(parents=True)
        binary_directory.mkdir(parents=True)
        debian.mkdir()

        for name in BINARIES:
            binary = binary_directory / name
            shutil.copyfile(source_binaries / name, binary)
            binary.chmod(0o755)
        notice_directory = installed / "notices"
        shutil.copytree(source / "notices", notice_directory, copy_function=shutil.copyfile)
        for path in notice_directory.rglob("*"):
            path.chmod(0o755 if path.is_dir() else 0o644)
        launcher = stage / "usr/bin/fframes-studio"
        launcher.write_text(
            "#!/bin/sh\n"
            "set -eu\n"
            'exec /opt/fframes-studio/bin/fframes-studio studio "$@"\n'
        )
        launcher.chmod(0o755)
        (stage / "usr/share/applications/fframes-studio.desktop").write_text(
            "[Desktop Entry]\n"
            "Type=Application\n"
            "Name=fframes Studio\n"
            "Comment=Native video authoring for fframes\n"
            "Exec=fframes-studio\n"
            "Terminal=false\n"
            "Categories=AudioVideo;Development;Graphics;\n"
            "StartupNotify=true\n"
        )

        control = debian / "control"
        control.write_text(
            "Package: fframes-studio\n"
            f"Version: {version}\n"
            "Section: video\n"
            "Priority: optional\n"
            "Architecture: amd64\n"
            f"Maintainer: {maintainer}\n"
            "Homepage: https://fframes.studio\n"
            "Depends: libc6\n"
            "Description: Native video authoring studio for fframes\n"
            " Desktop application for editing and exporting fframes projects.\n"
        )
        dependencies = deb_dependencies(stage)
        content_bytes = sum(
            path.stat().st_size
            for package_root in (installed, stage / "usr")
            for path in package_root.rglob("*")
            if path.is_file()
        )
        control.write_text(
            control.read_text().replace("Depends: libc6\n", f"Depends: {dependencies}\n")
            + f"Installed-Size: {(content_bytes + 1023) // 1024}\n"
        )
        run(["dpkg-deb", "--root-owner-group", "--build", str(stage), str(candidate)])
        try:
            os.link(candidate, output)
        except FileExistsError as error:
            raise ValueError(
                f"output appeared while packaging; existing bytes were preserved: {output}"
            ) from error
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="phase-zero native package directory")
    parser.add_argument("--out", type=Path, required=True, help="new .deb output path")
    parser.add_argument("--maintainer", required=True, help="package maintainer name and email")
    args = parser.parse_args()
    package(args.source, args.out, args.maintainer)


if __name__ == "__main__":
    main()
