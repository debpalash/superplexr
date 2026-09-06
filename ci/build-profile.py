#!/usr/bin/env python3
"""Build a selected development bundle; never run its executables or tests."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

if sys.version_info < (3, 11):
    raise SystemExit("Profile builds require Python 3.11+; invoke this script with python3.11 or newer.")

import tomllib


EXECUTABLES = {
    "ultraplexr": "ultraplexr-cli",
    "ultraplexr-server": "ultraplexr-server",
    "ultraplexr-desktop": "ultraplexr-desktop",
    "ultraplexr-tui": "ultraplexr-tui",
    "ultraplexr-observer": "ultraplexr-observer",
    "ultraplexr-mcp": "ultraplexr-mcp",
    "ultraplexr-agent-status-plugin": "ultraplexr-plugin",
}
PROFILES = {
    "host": ["ultraplexr", "ultraplexr-server"],
    "terminal": ["ultraplexr", "ultraplexr-tui"],
    "web": ["ultraplexr", "ultraplexr-observer"],
    "desktop": ["ultraplexr", "ultraplexr-server", "ultraplexr-desktop"],
    "automation": ["ultraplexr", "ultraplexr-server", "ultraplexr-mcp", "ultraplexr-agent-status-plugin"],
    "all": list(EXECUTABLES),
}


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def command_output(workspace, command):
    return subprocess.check_output(command, cwd=workspace, text=True).strip()


def build(workspace, command, names):
    artifacts = {}
    inputs = {}
    # Consume Cargo's emitted artifact paths, honoring target-dir overrides and
    # cross targets. Never infer success from an older file in target/release.
    with subprocess.Popen(command, cwd=workspace, stdout=subprocess.PIPE, text=True) as process:
        try:
            for line in process.stdout:
                message = json.loads(line)
                if message.get("reason") == "compiler-message":
                    rendered = message.get("message", {}).get("rendered")
                    if rendered:
                        sys.stderr.write(rendered)
                if message.get("reason") != "compiler-artifact":
                    continue
                package_id = message["package_id"]
                manifest_path = message.get("manifest_path")
                if not manifest_path:
                    raise RuntimeError(f"Cargo omitted the manifest path for {package_id}")
                record = inputs.setdefault(package_id, {
                    "package_id": package_id,
                    "manifest_path": manifest_path,
                    "features": set(),
                    "targets": set(),
                })
                record["features"].update(message.get("features", []))
                record["targets"].add((message.get("target", {}).get("name", ""), tuple(message.get("target", {}).get("kind", []))))
                target = message.get("target", {})
                name = target.get("name")
                executable = message.get("executable")
                if name in names and "bin" in target.get("kind", []) and executable:
                    if message.get("profile", {}).get("test"):
                        continue
                    artifacts[name] = {
                        "path": Path(executable),
                        "package_id": message["package_id"],
                        "features": message.get("features", []),
                        "fresh": message.get("fresh", False),
                    }
            status = process.wait()
            if status:
                raise subprocess.CalledProcessError(status, command)
        except BaseException:
            if process.poll() is None:
                process.terminate()
            raise
    missing = sorted(set(names) - artifacts.keys())
    if missing:
        raise RuntimeError(f"Cargo did not emit selected executable artifacts: {', '.join(missing)}")
    return artifacts, inputs


def package_evidence(record):
    manifest = Path(record["manifest_path"])
    with manifest.open("rb") as source:
        document = tomllib.load(source)
    package = document["package"]
    workspace_package = None
    workspace_root = None

    def field(name):
        nonlocal workspace_package, workspace_root
        value = package.get(name)
        if not isinstance(value, dict) or not value.get("workspace"):
            return value, manifest.parent
        if workspace_package is None:
            explicit = package.get("workspace")
            candidates = [manifest.parent / explicit] if explicit else [manifest.parent, *manifest.parent.parents]
            for root in candidates:
                candidate = root / "Cargo.toml"
                if not candidate.is_file():
                    continue
                with candidate.open("rb") as source:
                    owner = tomllib.load(source)
                if "workspace" in owner:
                    workspace_package = owner["workspace"].get("package", {})
                    workspace_root = root
                    break
            if workspace_package is None:
                raise RuntimeError(f"Could not resolve inherited package metadata for {manifest}")
        return workspace_package.get(name), workspace_root

    version, _ = field("version")
    license_name, license_owner = field("license")
    license_file, license_root = field("license-file")
    package_id = record["package_id"]
    source = package_id.split("#", 1)[0] if package_id.startswith(("registry+", "git+")) else None
    return {
        "id": package_id,
        "name": package["name"],
        "version": version,
        "source": source,
        "license": license_name,
        "license_file": str(license_root / license_file) if license_file else None,
        "license_file_root": str(license_root),
        "notice_roots": sorted({str(manifest.parent), str(license_owner)}),
        "manifest_path": str(manifest),
        "features": sorted(record["features"]),
        "compiled_targets": [{"name": name, "kinds": list(kinds)} for name, kinds in sorted(record["targets"])],
        "manifest_sha256": digest(manifest),
    }


def write_dependency_metadata(workspace, output, inputs, artifacts, profile, target, lock_digest):
    packages = [package_evidence(record) for _, record in sorted(inputs.items())]
    metadata = {
        "packages": packages,
        "workspace_members": sorted({artifact["package_id"] for artifact in artifacts.values()}),
        "resolve": {"nodes": []},
        "build_inputs_only": True,
        "source_description": "Generated from this profile's observed Cargo build artifacts, including build-time inputs; not the full workspace or a runtime-only graph.",
    }
    scope = {"profile": profile, "target": target, "lock": lock_digest, "packages": packages}
    scope_digest = hashlib.sha256(json.dumps(scope, sort_keys=True).encode("utf-8")).hexdigest()
    spec = importlib.util.spec_from_file_location("ultraplexr_release_metadata", workspace / "ci" / "release-metadata.py")
    if spec is None or spec.loader is None:
        raise RuntimeError("Cannot load the dependency metadata generator")
    generator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(generator)
    generator.generate_from_metadata(workspace, output, metadata, f"ultraplexr-{profile}-build-inputs", scope_digest)
    with (output / "build-inputs.json").open("x", encoding="utf-8") as destination:
        json.dump(scope, destination, indent=2)
        destination.write("\n")
    return [{"path": name, "bytes": (output / name).stat().st_size, "sha256": digest(output / name)}
            for name in ("ultraplexr.spdx.json", "THIRD_PARTY_NOTICES.txt", "build-inputs.json")]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=PROFILES)
    parser.add_argument("--output", type=Path, required=True, help="New bundle directory; existing paths are never overwritten")
    parser.add_argument("--target", help="Cargo target triple; requires its toolchain/linker and does not certify platform support")
    parser.add_argument("--online", action="store_true", help="Permit Cargo network access; default is --offline")
    parser.add_argument("--plan", action="store_true", help="Print the selection and command without executing Cargo or writing files")
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parent.parent
    output = args.output.absolute()
    names = PROFILES[args.profile]
    command = ["cargo", "build", "--release", "--locked", "--message-format=json-render-diagnostics", "--bins"]
    if not args.online:
        command.append("--offline")
    if args.target:
        command.extend(["--target", args.target])
    for package in sorted({EXECUTABLES[name] for name in names}):
        command.extend(["--package", package])
    if args.plan:
        print(json.dumps({"profile": args.profile, "executables": names, "command": command, "output": str(output)}, indent=2))
        return
    if output.exists() or output.is_symlink():
        raise RuntimeError(f"Refusing to overwrite existing bundle path: {output}")

    with (workspace / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["workspace"]["package"]["version"]
    rustc = command_output(workspace, ["rustc", "-vV"])
    host = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
    lock_digest = digest(workspace / "Cargo.lock")
    revision = command_output(workspace, ["git", "rev-parse", "HEAD"])
    dirty = bool(command_output(workspace, ["git", "status", "--porcelain", "--untracked-files=normal"]))
    artifacts, inputs = build(workspace, command, names)
    if digest(workspace / "Cargo.lock") != lock_digest:
        raise RuntimeError("Cargo.lock changed during the build; refusing to label the bundle")

    # Exclusive creation, no cleanup of user paths. A copy failure leaves a
    # partial directory without bundle.json; choose a new destination to retry.
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    (output / "bin").mkdir()
    records = []
    for name in names:
        artifact = artifacts[name]
        source = artifact["path"]
        suffix = ".exe" if source.suffix == ".exe" else ""
        destination = output / "bin" / (name + suffix)
        shutil.copy2(source, destination)
        records.append({
            "name": name,
            "path": str(destination.relative_to(output)),
            "bytes": destination.stat().st_size,
            "sha256": digest(destination),
            "package_id": artifact["package_id"],
            "features": artifact["features"],
            "cargo_fresh": artifact["fresh"],
        })
    shutil.copy2(workspace / "LICENSE", output / "LICENSE")
    terminfo = output / "share" / "terminfo"
    terminfo.mkdir(parents=True)
    shutil.copy2(workspace / "packaging" / "terminfo" / "xterm-ghostty.terminfo", terminfo)
    shutil.copy2(workspace / "docs" / "design" / "distribution-profiles.md", output / "README.md")
    dependency_files = write_dependency_metadata(workspace, output, inputs, artifacts, args.profile, args.target or host, lock_digest)
    metadata = {
        "schema_version": 1,
        "status": "development-bundle-unverified",
        "profile": args.profile,
        "version": version,
        "target": args.target or host,
        "rustc": rustc,
        "source_revision": revision,
        "source_dirty": dirty,
        "cargo_lock_sha256": lock_digest,
        "build_command": command,
        "build_environment": {key: os.environ.get(key) for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "SOURCE_DATE_EPOCH")},
        "executables": records,
        "dependency_metadata": dependency_files,
        "executable_bytes_total": sum(record["bytes"] for record in records),
        "qualification": "No executable was run. Cargo build-input SBOM/notices are not a runtime/system-library inventory or legal-completeness certification. Not signed, notarized or platform-certified.",
    }
    with (output / "bundle.json").open("x", encoding="utf-8") as manifest:
        json.dump(metadata, manifest, indent=2)
        manifest.write("\n")
    print(f"Development bundle written to {output}; verification remains separate.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Profile build failed: {error}", file=sys.stderr)
        raise SystemExit(1)
