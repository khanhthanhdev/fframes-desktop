#!/usr/bin/env python3
"""Refresh a native package inventory and archive after signing its binaries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import uuid


def _inventory_files(package: Path) -> list[dict[str, object]]:
    files = []
    for path in sorted(package.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"native package must not contain symlinks: {path.relative_to(package)}")
        if not path.is_file():
            continue
        relative = path.relative_to(package).as_posix()
        if relative == "inventory.json":
            continue
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        files.append({"path": relative, "size_bytes": path.stat().st_size, "sha256": digest})
    return files


def finalize(package_dir: Path) -> Path:
    if package_dir.is_symlink():
        raise ValueError("native package directory must not be a symlink")
    package = package_dir.resolve(strict=True)
    if not package.is_dir():
        raise ValueError("native package path must be a directory")

    qualification_path = package / "qualification.json"
    qualification = json.loads(qualification_path.read_text(encoding="utf-8"))
    target = qualification["target_platform"]["triple"]
    sdk_included = qualification["build_evidence"]["sdk_included"]
    if not isinstance(target, str) or not target or type(sdk_included) is not bool:
        raise ValueError("native package qualification identity is invalid")
    files = _inventory_files(package)

    archive = package.parent / f"{package.name}.zip"
    if archive.is_symlink() or not archive.is_file():
        raise ValueError("expected an existing regular native package archive")

    inventory_path = package / "inventory.json"
    if inventory_path.is_symlink():
        raise ValueError("native package inventory must not be a symlink")
    inventory = {
        "target": target,
        "sdk_included": sdk_included,
        "files": files,
    }
    temporary_inventory = package / f".inventory-{uuid.uuid4().hex}.tmp"
    temporary_archive_base = package.parent / f".{package.name}.{uuid.uuid4().hex}"
    temporary_archive = Path(f"{temporary_archive_base}.zip")
    try:
        with temporary_inventory.open("x", encoding="utf-8") as stream:
            json.dump(inventory, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary_inventory, inventory_path)

        generated_archive = Path(
            shutil.make_archive(str(temporary_archive_base), "zip", package.parent, package.name)
        )
        os.replace(generated_archive, archive)
    finally:
        temporary_inventory.unlink(missing_ok=True)
        temporary_archive.unlink(missing_ok=True)

    return archive


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package_dir", type=Path)
    args = parser.parse_args()
    print(finalize(args.package_dir))


if __name__ == "__main__":
    main()
