#!/usr/bin/env python3
"""Generate offline release metadata from Cargo's locked resolved graph."""

import argparse
import datetime
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path


def spdx_id(package_id, name):
    safe = re.sub(r"[^A-Za-z0-9.-]", "-", name)
    digest = hashlib.sha256(package_id.encode("utf-8")).hexdigest()[:12]
    return f"SPDXRef-Package-{safe}-{digest}"


def created_at():
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if epoch is not None:
        instant = datetime.datetime.fromtimestamp(int(epoch), datetime.timezone.utc)
    else:
        instant = datetime.datetime.now(datetime.timezone.utc)
    return instant.replace(microsecond=0).isoformat().replace("+00:00", "Z")


def cargo_metadata(workspace):
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=workspace,
        text=True,
    )
    return json.loads(output)


def external_refs(package):
    if (package.get("source") or "").startswith("registry+"):
        name = package["name"].replace("%", "%25").replace("/", "%2F")
        return [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": f"pkg:cargo/{name}@{package['version']}",
            }
        ]
    return []


def generate(workspace, output):
    metadata = cargo_metadata(workspace)
    generate_from_metadata(workspace, output, metadata)


def generate_from_metadata(workspace, output, metadata, name="superplexr-cargo-lock", scope_digest=None):
    """Render supplied package evidence without resolving another Cargo graph."""
    output.mkdir(parents=True, exist_ok=True)
    lock_digest = hashlib.sha256((workspace / "Cargo.lock").read_bytes()).hexdigest()
    identifiers = {
        package["id"]: spdx_id(package["id"], package["name"])
        for package in metadata["packages"]
    }
    packages = []
    for package in sorted(metadata["packages"], key=lambda item: item["id"]):
        item = {
            "SPDXID": identifiers[package["id"]],
            "name": package["name"],
            "versionInfo": package["version"],
            "downloadLocation": package.get("source") or "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": package.get("license") or "NOASSERTION",
            "copyrightText": "NOASSERTION",
        }
        refs = external_refs(package)
        if refs:
            item["externalRefs"] = refs
        packages.append(item)

    relationships = []
    workspace_members = set(metadata["workspace_members"])
    for package_id in sorted(workspace_members):
        relationships.append(
            {
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "DESCRIBES",
                "relatedSpdxElement": identifiers[package_id],
            }
        )
    nodes = metadata.get("resolve", {}).get("nodes", [])
    for node in sorted(nodes, key=lambda item: item["id"]):
        for dependency in sorted(node["dependencies"]):
            relationships.append(
                {
                    "spdxElementId": identifiers[node["id"]],
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": identifiers[dependency],
                }
            )

    if metadata.get("build_inputs_only"):
        for package_id in sorted(identifiers):
            relationships.append({
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "CONTAINS",
                "relatedSpdxElement": identifiers[package_id],
            })

    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": name,
        "documentNamespace": f"https://superplexr.local/spdx/{scope_digest or lock_digest}",
        "creationInfo": {
            "created": created_at(),
            "creators": ["Organization: superplexr contributors"],
        },
        "packages": packages,
        "relationships": relationships,
    }
    if metadata.get("build_inputs_only"):
        document["comment"] = "Observed Cargo build inputs, including build scripts and procedural macros; not a runtime-only dependency graph or system-library inventory."
    (output / "superplexr.spdx.json").write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    generate_notices(metadata, output / "THIRD_PARTY_NOTICES.txt")


def generate_notices(metadata, destination):
    lines = [
        "superplexr third-party dependency and notice bundle",
        metadata.get("source_description", "Generated from Cargo's locked resolved graph."),
        "",
        "Dependency inventory",
        "====================",
    ]
    packages = sorted(metadata["packages"], key=lambda item: (item["name"], item["version"], item["id"]))
    for package in packages:
        source = package.get("source") or "workspace/local source"
        license_name = package.get("license") or "NOASSERTION"
        lines.append(f"{package['name']} {package['version']} | {license_name} | {source}")

    license_groups = {}
    patterns = ("LICENSE*", "COPYING*", "NOTICE*")
    for package in packages:
        root = Path(package["manifest_path"]).parent
        candidates = set()
        for notice_root in package.get("notice_roots", [str(root)]):
            notice_root = Path(notice_root).resolve()
            for pattern in patterns:
                candidates.update(path for path in notice_root.glob(pattern)
                                  if path.resolve().is_relative_to(notice_root))
        explicit = package.get("license_file")
        if explicit:
            path = Path(explicit)
            if not path.is_absolute():
                path = root / path
            allowed_root = Path(package.get("license_file_root", root)).resolve()
            if path.resolve().is_relative_to(allowed_root):
                candidates.add(path)
        for path in sorted(candidates):
            if not path.is_file():
                continue
            content = path.read_bytes()
            digest = hashlib.sha256(content).hexdigest()
            group = license_groups.setdefault(
                digest,
                {"content": content.decode("utf-8", errors="replace"), "packages": []},
            )
            label = f"{package['name']} {package['version']} ({path.name})"
            if label not in group["packages"]:
                group["packages"].append(label)

    lines.extend(["", "Collected license and notice texts", "=================================="])
    for digest, group in sorted(license_groups.items()):
        lines.extend(
            [
                "",
                f"SHA-256: {digest}",
                "Applies to: " + ", ".join(sorted(group["packages"])),
                "-" * 78,
                group["content"].rstrip(),
            ]
        )
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")


def checksums(output):
    entries = []
    for path in sorted(output.iterdir(), key=lambda item: item.name):
        if not path.is_file() or path.name == "SHA256SUMS":
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        entries.append(f"{digest}  {path.name}")
    (output / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="ascii")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("generate", "checksums"))
    parser.add_argument("--workspace", type=Path, default=Path("."))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    workspace = args.workspace.resolve()
    output = args.output.resolve()
    if args.mode == "generate":
        generate(workspace, output)
    else:
        output.mkdir(parents=True, exist_ok=True)
        checksums(output)


if __name__ == "__main__":
    main()
