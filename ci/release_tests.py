"""Exercise publication guards and real archive assembly without a native build."""

import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import tomllib
import unittest

SPEC = importlib.util.spec_from_file_location("release_packager", Path(__file__).with_name("package-release.py"))
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class ReleaseTests(unittest.TestCase):
    def test_all_profile_covers_every_cargo_binary(self):
        workspace = tomllib.loads((RELEASE.WORKSPACE / "Cargo.toml").read_text())
        binaries = {}
        for member in workspace["workspace"]["members"]:
            root = RELEASE.WORKSPACE / member
            manifest = tomllib.loads((root / "Cargo.toml").read_text())
            name = manifest["package"]["name"]
            explicit = manifest.get("bin", [])
            binaries.update({binary["name"]: name for binary in explicit})
            if (root / "src/main.rs").exists() and not explicit:
                binaries[name] = name
        self.assertEqual(RELEASE.BUILDER.EXECUTABLES, binaries)

    def bundle(self, root):
        bundle = root / "input"
        (bundle / "bin").mkdir(parents=True)
        records = []
        for name in RELEASE.BUILDER.EXECUTABLES:
            path = bundle / "bin" / name
            path.write_bytes(b"fixture executable\n")
            path.chmod(0o755)
            records.append({"name": name, "path": f"bin/{name}", "bytes": path.stat().st_size,
                            "sha256": RELEASE.BUILDER.digest(path)})
        for name in ("LICENSE", "THIRD_PARTY_NOTICES.txt", "superplexr.spdx.json"):
            (bundle / name).write_text("fixture\n")
        manifest = {"target": "x86_64-unknown-linux-gnu", "version": RELEASE.version(),
                    "source_dirty": False, "executables": records, "dependency_metadata": []}
        (bundle / "bundle.json").write_text(json.dumps(manifest))
        return bundle, manifest

    def test_archive_preserves_all_binaries_permissions_and_terminfo(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, manifest = self.bundle(root)
            output = root / "output"
            RELEASE.package(bundle, manifest["target"], output)
            archive_path, = output.iterdir()
            with tarfile.open(archive_path) as archive:
                prefix = RELEASE.archive_name(manifest["target"])
                for name in RELEASE.BUILDER.EXECUTABLES:
                    member = archive.getmember(f"{prefix}/bin/{name}")
                    self.assertEqual(member.mode & 0o111, 0o111)
                self.assertTrue(any(member.name.endswith("/xterm-ghostty") for member in archive))
                self.assertIn("GPUI experiment", archive.extractfile(f"{prefix}/README.md").read().decode())
            with self.assertRaises(FileExistsError):
                RELEASE.package(bundle, manifest["target"], output)

    def test_tampered_missing_dirty_and_wrong_target_bundles_are_rejected(self):
        for fault in ("tampered", "missing", "dirty", "target"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                bundle, manifest = self.bundle(root)
                if fault == "tampered":
                    (bundle / "bin/superplexr").write_text("changed")
                elif fault == "missing":
                    manifest["executables"].pop()
                elif fault == "dirty":
                    manifest["source_dirty"] = True
                else:
                    manifest["target"] = "aarch64-apple-darwin"
                (bundle / "bundle.json").write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    RELEASE.package(bundle, "x86_64-unknown-linux-gnu", root / "output")
                self.assertFalse((root / "output").exists())

    def test_incomplete_release_cannot_get_checksums(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            for target in RELEASE.TARGETS:
                (output / f"{RELEASE.archive_name(target)}.tar.gz").write_bytes(b"fixture")
            with self.assertRaises(ValueError):
                RELEASE.verify_release(output)
            self.assertFalse((output / "SHA256SUMS").exists())
            for target in RELEASE.TARGETS:
                if target.endswith("apple-darwin"):
                    (output / f"{RELEASE.archive_name(target)}.app.zip").write_bytes(b"fixture")
            RELEASE.verify_release(output)
            self.assertEqual(len((output / "SHA256SUMS").read_text().splitlines()), 6)


if __name__ == "__main__":
    unittest.main()
