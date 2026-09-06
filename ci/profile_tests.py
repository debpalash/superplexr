"""Profile selection and artifact-admission regressions; no Cargo or app launch."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build-profile.py")
SPEC = importlib.util.spec_from_file_location("profile_builder", SCRIPT)
BUILDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILDER)


class ProfileTests(unittest.TestCase):
    def test_every_plan_is_explicit_locked_offline_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as root:
            for profile, names in BUILDER.PROFILES.items():
                output = Path(root) / profile
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), profile, "--output", str(output), "--plan"],
                    check=True, capture_output=True, text=True,
                )
                plan = json.loads(result.stdout)
                self.assertEqual(plan["executables"], names)
                self.assertFalse(output.exists())
                command = plan["command"]
                for option in ["--release", "--locked", "--offline", "--bins"]:
                    self.assertIn(option, command)
                packages = {command[index + 1] for index, item in enumerate(command) if item == "--package"}
                self.assertEqual(packages, {BUILDER.EXECUTABLES[name] for name in names})
                if profile in {"host", "terminal", "web", "automation"}:
                    self.assertNotIn("ultraplexr-desktop", packages)

    def test_explicit_cross_target_and_online_only_change_the_plan(self):
        with tempfile.TemporaryDirectory() as root:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "terminal", "--output", str(Path(root) / "bundle"),
                 "--target", "x86_64-unknown-linux-gnu", "--online", "--plan"],
                check=True, capture_output=True, text=True,
            )
            command = json.loads(result.stdout)["command"]
            self.assertNotIn("--offline", command)
            self.assertEqual(command[command.index("--target") + 1], "x86_64-unknown-linux-gnu")
            self.assertEqual(list(Path(root).iterdir()), [])

    def test_existing_path_and_dangling_symlink_are_refused_before_cargo(self):
        with tempfile.TemporaryDirectory() as root:
            occupied = Path(root) / "occupied"
            occupied.write_text("user-owned contents")
            link = Path(root) / "dangling"
            link.symlink_to(Path(root) / "missing")
            for output in (occupied, link):
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), "host", "--output", str(output)],
                    capture_output=True, text=True,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Refusing to overwrite", result.stderr)
            self.assertEqual(occupied.read_text(), "user-owned contents")
            self.assertTrue(link.is_symlink())

    @staticmethod
    def artifact(name="ultraplexr", **updates):
        record = {
            "reason": "compiler-artifact", "package_id": "path+file:///fixture#ultraplexr-cli@0.1.0",
            "manifest_path": "/fixture/Cargo.toml", "features": ["one"],
            "target": {"name": name, "kind": ["bin"]},
            "profile": {"test": False}, "executable": "/custom/target/ultraplexr", "fresh": True,
        }
        record.update(updates)
        return record

    @staticmethod
    def emit(records, exit_code=0):
        # A tiny protocol fixture, not Cargo or a built application. Emit each
        # supplied JSON record exactly as Cargo's stdout stream would.
        return [sys.executable, "-c",
                "import json,sys; [print(json.dumps(x)) for x in json.loads(sys.argv[1])]; sys.exit(int(sys.argv[2]))",
                json.dumps(records), str(exit_code)]

    def test_only_emitted_non_test_executables_supply_paths_and_freshness(self):
        records = [self.artifact(profile={"test": True}, executable="/wrong/test"), self.artifact()]
        artifacts, inputs = BUILDER.build(SCRIPT.parent, self.emit(records), ["ultraplexr"])
        self.assertEqual(artifacts["ultraplexr"]["path"], Path("/custom/target/ultraplexr"))
        self.assertTrue(artifacts["ultraplexr"]["fresh"])
        self.assertEqual(len(inputs), 1)

    def test_failed_build_never_admits_even_emitted_executables(self):
        with self.assertRaises(subprocess.CalledProcessError):
            BUILDER.build(SCRIPT.parent, self.emit([self.artifact()], 1), ["ultraplexr"])

    def test_missing_executable_or_manifest_cannot_use_stale_guessed_paths(self):
        for records in [[], [self.artifact(profile={"test": True})], [self.artifact(manifest_path=None)]]:
            with self.subTest(records=records), self.assertRaises(RuntimeError):
                BUILDER.build(SCRIPT.parent, self.emit(records), ["ultraplexr"])

    def test_input_features_and_targets_are_unioned_for_emitted_package(self):
        records = [self.artifact(), self.artifact(features=["two"], target={"name": "library", "kind": ["lib"]}, executable=None)]
        _, inputs = BUILDER.build(SCRIPT.parent, self.emit(records), ["ultraplexr"])
        record = next(iter(inputs.values()))
        self.assertEqual(record["features"], {"one", "two"})
        self.assertEqual(record["targets"], {("ultraplexr", ("bin",)), ("library", ("lib",))})

    def test_package_evidence_resolves_inherited_fields_without_full_metadata(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["member"]\n[workspace.package]\nversion="1.2.3"\nlicense="MIT"\n')
            member = root / "member"
            member.mkdir()
            manifest = member / "Cargo.toml"
            manifest.write_text('[package]\nname="member"\nversion.workspace=true\nlicense.workspace=true\n')
            evidence = BUILDER.package_evidence({"manifest_path": str(manifest), "package_id": "local-member",
                                                "features": {"b", "a"}, "targets": {("member", ("lib",))}})
            self.assertEqual(evidence["version"], "1.2.3")
            self.assertEqual(evidence["license"], "MIT")
            self.assertEqual(evidence["features"], ["a", "b"])
            self.assertEqual(evidence["manifest_sha256"], BUILDER.digest(manifest))
            self.assertIn(str(root), evidence["notice_roots"])


if __name__ == "__main__":
    unittest.main()
