#!/usr/bin/env python3
"""Package verified native bundles and require every target before publication."""

import argparse
import importlib.util
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

WORKSPACE = Path(__file__).resolve().parent.parent
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin",
           "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu")
SPEC = importlib.util.spec_from_file_location("profile_builder", WORKSPACE / "ci/build-profile.py")
BUILDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILDER)


def version():
    return tomllib.loads((WORKSPACE / "Cargo.toml").read_text())["workspace"]["package"]["version"]


def archive_name(target):
    return f"superplexr-v{version()}-{target}"


def validate_bundle(bundle, target):
    manifest = json.loads((bundle / "bundle.json").read_text())
    if manifest["target"] != target or manifest["version"] != version():
        raise ValueError("Bundle target/version does not match this release")
    if manifest["source_dirty"]:
        raise ValueError("Refusing to release a dirty source build")
    records = manifest["executables"]
    if len(records) != len(BUILDER.EXECUTABLES) or {r["name"] for r in records} != set(BUILDER.EXECUTABLES):
        raise ValueError("Release must contain every workspace executable exactly once")
    for record in records + manifest["dependency_metadata"]:
        path = bundle / record["path"]
        if not path.resolve().is_relative_to(bundle.resolve()) or not path.is_file():
            raise ValueError("Invalid bundle file path")
        if path.stat().st_size != record["bytes"] or BUILDER.digest(path) != record["sha256"]:
            raise ValueError(f"Bundle checksum mismatch: {record['path']}")
    return manifest


def package(bundle, target, output):
    validate_bundle(bundle, target)
    output.mkdir(parents=True, exist_ok=False)
    # Work on a private copy: input manifest hashes remain valid and a failed
    # packaging operation never mutates the original build evidence.
    with tempfile.TemporaryDirectory(prefix="superplexr-release-") as temporary:
        root = Path(temporary)
        stage = root / archive_name(target)
        shutil.copytree(bundle, stage)
        shutil.copy2(WORKSPACE / "docs/release-notes.md", stage / "README.md")
        (stage / "share/terminfo").mkdir(parents=True, exist_ok=True)
        subprocess.run(["tic", "-x", "-o", str(stage / "share/terminfo"),
                        str(WORKSPACE / "packaging/terminfo/xterm-ghostty.terminfo")], check=True)
        subprocess.run(["infocmp", "-A", str(stage / "share/terminfo"), "xterm-ghostty"],
                       check=True, stdout=subprocess.DEVNULL)
        with tarfile.open(output / f"{stage.name}.tar.gz", "w:gz") as archive:
            archive.add(stage, arcname=stage.name)
        if target.endswith("apple-darwin"):
            app = root / "SuperPlexr.app"
            macos = app / "Contents/MacOS"
            resources = app / "Contents/Resources"
            macos.mkdir(parents=True)
            resources.mkdir()
            for binary in BUILDER.EXECUTABLES:
                name = {"superplexr": "superplexr-cli", "superplexr-desktop": "superplexr"}.get(binary, binary)
                shutil.copy2(stage / "bin" / binary, macos / name)
            info = plistlib.loads((WORKSPACE / "packaging/macos/Info.plist").read_bytes())
            info["CFBundleShortVersionString"] = version()
            info["CFBundleVersion"] = version()
            (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
            shutil.copy2(WORKSPACE / "packaging/macos/SuperPlexr.icns", resources)
            shutil.copytree(stage / "share/terminfo", resources / "terminfo")
            for name in ("LICENSE", "THIRD_PARTY_NOTICES.txt", "superplexr.spdx.json"):
                shutil.copy2(stage / name, resources)
            subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
            subprocess.run(["ditto", "-c", "-k", "--norsrc", "--keepParent", str(app),
                            str(output / f"{stage.name}.app.zip")], check=True)


def verify_release(output):
    expected = {f"{archive_name(target)}.tar.gz" for target in TARGETS}
    expected.update(f"{archive_name(target)}.app.zip" for target in TARGETS if target.endswith("apple-darwin"))
    actual = {path.name for path in output.iterdir() if path.name != "SHA256SUMS"}
    if actual != expected:
        raise ValueError(f"Incomplete release: missing={expected - actual}, unexpected={actual - expected}")
    lines = []
    for name in sorted(expected):
        path = output / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Empty release artifact: {name}")
        lines.append(f"{BUILDER.digest(path)}  {name}\n")
    (output / "SHA256SUMS").write_text("".join(lines))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--verify-release", type=Path)
    args = parser.parse_args()
    if args.verify_release:
        verify_release(args.verify_release)
    elif args.bundle and args.target and args.output:
        package(args.bundle, args.target, args.output)
    else:
        parser.error("Use --bundle/--target/--output or --verify-release")
