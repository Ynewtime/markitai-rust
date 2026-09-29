"""Offline license-provenance counterexamples; never compile or download."""
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from license_overlay import stage_overlay
from package_go_static import bundle_licenses, inventory, package_archive, unpack_verified

VENDOR = Path(__file__).resolve().parents[1] / "licenses/upstream"


class LicenseOverlayTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repository = self.root / "repository"
        self.vendor = self.repository / "licenses/upstream"
        shutil.copytree(VENDOR, self.vendor)
        self.manifest = json.loads((self.vendor / "manifest.json").read_text())
        self.packages = []
        for item in self.manifest["packages"]:
            label = f"{item['name']}-{item['version']}"
            source = self.root / "registry" / label
            source.mkdir(parents=True)
            for name in [".cargo_vcs_info.json", "Cargo.toml.orig"]:
                shutil.copyfile(self.vendor / "local-evidence" / label / name, source / name)
            self.packages.append({"id": item["id"], "name": item["name"], "version": item["version"],
                                  "source": "registry+fixture", "license": item["declared_license"],
                                  "license_file": None, "repository": item["repository"],
                                  "manifest_path": str(source / "Cargo.toml")})
        self.destination = self.root / "module/licenses/upstream"

    def seal(self, name):
        """Rehash one changed test input to exercise semantic checks separately."""
        raw = (self.vendor / name).read_bytes()
        path = self.vendor / "inventory.json"
        value = json.loads(path.read_text())
        row = next(entry for entry in value["files"] if entry["path"] == name)
        row.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        path.write_text(json.dumps(value))

    def save_manifest(self):
        (self.vendor / "manifest.json").write_text(json.dumps(self.manifest))
        self.seal("manifest.json")

    def test_complete_text_and_notice_are_distinct_and_all_provenance_is_archived(self):
        ffi = self.repository / "crates/ffi"
        ffi.mkdir(parents=True)
        for name in ["LICENSE", "NOTICE"]:
            (self.repository / name).write_text("Fixture root notice\n")
        root_package = {"id": "fixture-ffi", "name": "markitai-ffi", "version": "1",
                        "source": None, "license": "MIT", "manifest_path": str(ffi / "Cargo.toml")}
        metadata = {"packages": [root_package] + self.packages,
                    "resolve": {"nodes": [{"id": "fixture-ffi", "deps": [{"pkg": p["id"]} for p in self.packages]}]
                                + [{"id": p["id"], "deps": []} for p in self.packages]}}
        sysroot = self.root / "rust"
        toolchain = sysroot / "share/doc/rust/LICENSE-test"
        toolchain.parent.mkdir(parents=True)
        toolchain.write_text("Fixture toolchain notice\n")
        self.destination.parent.parent.mkdir(parents=True)
        record = bundle_licenses(metadata, self.destination.parent, self.repository, sysroot)
        self.assertEqual(record["legal_review"], "not_performed")
        self.assertEqual(record["upstream_overlay"]["complete_text_packages"], 6)
        self.assertEqual(record["upstream_overlay"]["notice_only_packages"], 9)
        expected_missing = {p["id"] for p in self.manifest["packages"] if p["content_kind"] == "notice_only"}
        self.assertEqual({p["id"] for p in record["unresolved"]}, expected_missing)
        self.assertEqual(len(expected_missing), 9)
        copied = [text for p in record["dependencies"] for text in p["texts"] if text.get("origin") == "verified_upstream_overlay"]
        self.assertEqual(len(copied), 16)
        for text in copied:
            raw = (self.destination.parent.parent / text["path"]).read_bytes()
            self.assertEqual(raw, Path(text["source"]).read_bytes())
            self.assertEqual(hashlib.sha256(raw).hexdigest(), text["sha256"])
        self.assertEqual(inventory(self.vendor), inventory(self.destination))
        archive = self.root / "licenses.tgz"
        expected = package_archive(self.destination.parent.parent, archive)
        unpack_verified(archive, self.root / "installed", expected)
        self.assertEqual(inventory(self.root / "installed/licenses/upstream"), inventory(self.vendor))

    def test_a_new_version_cannot_inherit_old_version_license_evidence(self):
        future = dict(self.packages[0], id="registry+fixture#alloc-stdlib@99.0", version="99.0")
        result = stage_overlay(self.vendor, self.destination, [future])
        self.assertEqual(result["packages"], {})
        self.assertEqual(result["record"]["matched_packages"], 0)

    def test_current_package_vcs_original_and_license_must_match(self):
        for kind in ["vcs", "original", "license"]:
            with self.subTest(kind=kind):
                package = dict(self.packages[0])
                directory = Path(package["manifest_path"]).parent
                name = ".cargo_vcs_info.json" if kind == "vcs" else "Cargo.toml.orig"
                previous = (directory / name).read_bytes()
                if kind == "license":
                    package["license"] = "MIT"
                else:
                    (directory / name).write_bytes(previous + b"\n")
                with self.assertRaisesRegex(RuntimeError, "differs"):
                    stage_overlay(self.vendor, self.destination, [package])
                self.assertFalse(self.destination.exists())
                (directory / name).write_bytes(previous)

    def test_changed_asset_bytes_fail_before_copying(self):
        asset = self.manifest["packages"][0]["assets"][0]["path"]
        (self.vendor / asset).write_bytes(b"substituted text")
        with self.assertRaisesRegex(RuntimeError, "identity differs"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())
        self.seal(asset)
        with self.assertRaisesRegex(RuntimeError, "identity differs"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_notice_classification_and_upstream_url_are_checked(self):
        notice = next(p for p in self.manifest["packages"] if p["content_kind"] == "notice_only")
        notice["content_kind"] = "full_license_text"
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "cannot claim"):
            stage_overlay(self.vendor, self.destination, self.packages)
        notice["content_kind"] = "notice_only"
        notice["assets"][0]["source_url"] = "https://raw.githubusercontent.com/example/other/main/LICENSE"
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "URL or hash"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_notice_extraction_must_equal_exact_source_byte_range(self):
        selector = next(p for p in self.manifest["packages"] if p["name"] == "selectors")
        selector["assets"][0]["source_byte_range"]["end_exclusive"] -= 1
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "source bytes"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_inventory_rejects_symlinks_traversal_and_unlisted_files(self):
        (self.vendor / "unreviewed.txt").write_text("extra")
        with self.assertRaisesRegex(RuntimeError, "not exact"):
            stage_overlay(self.vendor, self.destination, self.packages)
        (self.vendor / "unreviewed.txt").unlink()
        source = self.vendor / "README.md"
        outside = self.root / "readme"
        source.rename(outside)
        source.symlink_to(outside)
        with self.assertRaisesRegex(RuntimeError, "symlinks"):
            stage_overlay(self.vendor, self.destination, self.packages)
        source.unlink()
        outside.rename(source)
        value = json.loads((self.vendor / "inventory.json").read_text())
        value["files"][0]["path"] = "../escape"
        (self.vendor / "inventory.json").write_text(json.dumps(value))
        with self.assertRaisesRegex(RuntimeError, "escapes"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_boolean_sizes_and_duplicate_manifest_keys_are_not_accepted(self):
        value = json.loads((self.vendor / "inventory.json").read_text())
        previous = json.dumps(value)
        value["files"][0]["bytes"] = True
        (self.vendor / "inventory.json").write_text(json.dumps(value))
        with self.assertRaisesRegex(RuntimeError, "identity"):
            stage_overlay(self.vendor, self.destination, self.packages)
        (self.vendor / "inventory.json").write_text(previous)
        raw = (self.vendor / "manifest.json").read_text()
        (self.vendor / "manifest.json").write_text(raw.replace('"schema": 1,', '"schema": 1, "schema": 1,', 1))
        self.seal("manifest.json")
        with self.assertRaisesRegex(RuntimeError, "Duplicate"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
