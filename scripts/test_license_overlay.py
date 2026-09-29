"""Offline license-provenance counterexamples; never compile or download."""
import hashlib
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import tarfile
import zipfile

from license_overlay import stage_overlay, upstream_files
from ci_packages import supplement_wheel_licenses, verify_node_licenses
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
        self.assertEqual(record["upstream_overlay"]["complete_text_packages"], 15)
        self.assertEqual(record["upstream_overlay"]["notice_only_packages"], 0)
        self.assertEqual(record["unresolved"], [])
        self.assertEqual(record["upstream_overlay"]["historical_text_packages"], 3)
        self.assertEqual(record["upstream_overlay"]["legal_review"], "not_performed")
        historical = [p for p in record["dependencies"]
                      if p.get("upstream_evidence", {}).get("historical_provenance")]
        self.assertEqual({p["name"] for p in historical}, {"objc2", "objc2-encode", "objc2-foundation"})
        copied = [text for p in record["dependencies"] for text in p["texts"] if text.get("origin") == "verified_upstream_overlay"]
        self.assertEqual(len(copied), 22)
        for text in copied:
            raw = (self.destination.parent.parent / text["path"]).read_bytes()
            self.assertEqual(raw, Path(text["source"]).read_bytes())
            self.assertEqual(hashlib.sha256(raw).hexdigest(), text["sha256"])
        self.assertEqual(inventory(self.vendor), inventory(self.destination))
        archive = self.root / "licenses.tgz"
        expected = package_archive(self.destination.parent.parent, archive)
        unpack_verified(archive, self.root / "installed", expected)
        self.assertEqual(inventory(self.root / "installed/licenses/upstream"), inventory(self.vendor))

    def test_current_summary_is_derived_while_collection_history_stays_separate(self):
        self.assertEqual(self.manifest["unresolved_full_license_text"], [])
        historical = self.manifest["historical_collection"]["unresolved_full_license_text"]
        self.assertEqual(len(historical), 9)
        self.assertEqual(self.manifest["historical_collection"]["source"], "input-manifest.json")
        result = stage_overlay(self.vendor, self.destination, self.packages)
        self.assertEqual(result["record"]["complete_text_packages"], 15)
        self.assertEqual(result["record"]["historical_text_packages"], 3)
        self.assertEqual(result["record"]["legal_review"], "not_performed")
        copied = json.loads((self.destination / "manifest.json").read_text())
        self.assertEqual(copied["historical_collection"]["unresolved_full_license_text"], historical)
        self.assertEqual(copied["unresolved_full_license_text"], [])

    def test_resealed_stale_unresolved_or_count_summary_fails_before_copying(self):
        original = json.loads(json.dumps(self.manifest))
        variants = [("unresolved", None), ("historical_text_packages", 0),
                    ("full_license_text_packages", 14), ("notice_only_packages", 3),
                    ("requested_packages", 14), ("exact_commit_manifest_matches", 14),
                    ("overlay_files", 24), ("source_archives", True)]
        for field, value in variants:
            with self.subTest(field=field):
                self.manifest = json.loads(json.dumps(original))
                if field == "unresolved":
                    self.manifest["unresolved_full_license_text"] = self.manifest["historical_collection"]["unresolved_full_license_text"]
                else:
                    self.manifest["summary"][field] = value
                self.save_manifest()
                with self.assertRaisesRegex(RuntimeError, "summary differs"):
                    stage_overlay(self.vendor, self.destination, self.packages)
                self.assertFalse(self.destination.exists())

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
        notice = next(p for p in self.manifest["packages"] if p["name"] == "objc2")
        notice["complete_license_options_present"] = []
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "cannot claim"):
            stage_overlay(self.vendor, self.destination, self.packages)
        notice["complete_license_options_present"] = ["MIT"]
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


    def authority(self):
        entry = next(p for p in self.manifest["packages"] if p["name"] == "dispatch2")
        asset = next(a for a in entry["assets"] if a.get("source_kind") == "linked_authority")
        return entry, asset

    def test_authority_url_and_license_option_are_not_general_allowlists(self):
        entry, asset = self.authority()
        for field, value in [("source_url", "https://www.apache.org.example/LICENSE-2.0.txt"),
                             ("source_url", "https://www.apache.org/licenses/LICENSE-2.0.txt?variant=other"),
                             ("license_option", "MIT"),
                             ("source_kind", "external_url"),
                             ("notice_asset", "overlay/licenses/selectors-0.38.0/NOTICE-MPL-2.0.txt")]:
            with self.subTest(field=field, value=value):
                old = asset.get(field)
                asset[field] = value
                self.save_manifest()
                with self.assertRaises(RuntimeError):
                    stage_overlay(self.vendor, self.destination, self.packages)
                self.assertFalse(self.destination.exists())
                asset[field] = old
        self.save_manifest()

    def test_resealed_authority_substitution_still_fails_reviewed_digest(self):
        _, asset = self.authority()
        raw = (self.vendor / asset["source_path"]).read_bytes() + b"\nSubstituted terms\n"
        for name in [asset["source_path"], asset["path"]]:
            (self.vendor / name).write_bytes(raw)
            self.seal(name)
        asset.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                     source_sha256=hashlib.sha256(raw).hexdigest())
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "reviewed text hash"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_authority_requires_the_link_in_the_verified_version_notice(self):
        entry, _ = self.authority()
        notice = entry["assets"][0]
        raw = (self.vendor / notice["source_path"]).read_bytes().replace(
            b"[Apache-2.0]: https://www.apache.org/licenses/LICENSE-2.0",
            b"[Apache-2.0]: https://example.invalid/unreviewed")
        for name in [notice["source_path"], notice["path"]]:
            (self.vendor / name).write_bytes(raw)
            self.seal(name)
        notice.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                      source_sha256=hashlib.sha256(raw).hexdigest())
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "does not link"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_historical_original_closes_text_gap_without_claiming_legal_review(self):
        result = stage_overlay(self.vendor, self.destination, self.packages)
        self.assertEqual(result["record"]["historical_text_packages"], 3)
        self.assertEqual(result["record"]["legal_review"], "not_performed")
        for entry in self.manifest["packages"]:
            if not entry.get("supplemental"):
                continue
            record = result["packages"][entry["id"]]
            self.assertTrue(record["complete_text"])
            self.assertTrue(record["historical_provenance"])
            self.assertIsNone(record["full_text_gap"])
            supplemental = next(t for t in record["texts"] if t.get("source_kind") == "historical_notice")
            self.assertTrue(supplemental["provenance_verified"])
            self.assertEqual(supplemental["legal_review"], "not_performed")
            self.assertIn(b"Copyright (c) Steven Sheldon", Path(supplemental["source"]).read_bytes())
        shutil.rmtree(self.destination)
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        entry["supplemental"][0]["legal_review"] = "approved"
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "reviewed scope"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_historical_scope_cannot_expand_to_other_versions_or_current_commit(self):
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        asset = entry["supplemental"][0]
        for target, field, value in [(entry, "version", "99.0"),
                                     (entry, "declared_license", "Apache-2.0"),
                                     (asset, "source_kind", "same_commit"),
                                     (asset, "provenance_verified", False),
                                     (asset, "proofs", asset["proofs"][:-1])]:
            with self.subTest(field=field):
                old = target[field]
                target[field] = value
                self.save_manifest()
                with self.assertRaises(RuntimeError):
                    stage_overlay(self.vendor, self.destination, self.packages)
                self.assertFalse(self.destination.exists())
                target[field] = old
        self.save_manifest()

    def test_resealed_historical_terms_cannot_replace_original_copyright(self):
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        asset = entry["supplemental"][0]
        raw = (self.vendor / asset["source_path"]).read_bytes().replace(
            b"Copyright (c) Steven Sheldon", b"Copyright (c) <year> <copyright holders>")
        for name in [asset["source_path"], asset["path"]]:
            (self.vendor / name).write_bytes(raw)
            self.seal(name)
        asset.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                     source_sha256=hashlib.sha256(raw).hexdigest())
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "Historical original URL or reviewed text hash"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_resealed_current_notice_cannot_drop_apple_sdk_discussion(self):
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        notice = entry["assets"][0]
        raw = (self.vendor / notice["source_path"]).read_bytes().split(b"## Apple SDKs")[0]
        # The pinned root notice is shared by dispatch2 and objc2. Reseal all
        # references so rejection tests the reviewed historical notice, not a
        # stale digest in an earlier package.
        (self.vendor / notice["source_path"]).write_bytes(raw)
        self.seal(notice["source_path"])
        for package in self.manifest["packages"]:
            for asset in package["assets"]:
                if asset.get("source_path") == notice["source_path"]:
                    (self.vendor / asset["path"]).write_bytes(raw)
                    self.seal(asset["path"])
                    asset.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                                 source_sha256=hashlib.sha256(raw).hexdigest())
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "current-version MIT notice differs"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_current_package_mit_declaration_is_required_even_with_historical_text(self):
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        suffix = entry["path_in_vcs"] + "/Cargo.toml"
        upstream = next(p for p in entry["evidence"] if p["path"].startswith("upstream/") and p["path"].endswith(suffix))
        original = next(p for p in entry["evidence"] if p["path"].endswith("/Cargo.toml.orig"))
        raw = (self.vendor / upstream["path"]).read_bytes().replace(b'license = "MIT"', b'license = "Apache-2.0"')
        for proof in [upstream, original]:
            (self.vendor / proof["path"]).write_bytes(raw)
            proof.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
            self.seal(proof["path"])
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "current package MIT declaration"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_missing_or_resealed_historical_provenance_is_not_accepted(self):
        entry = next(p for p in self.manifest["packages"] if p.get("supplemental"))
        proof = entry["supplemental"][0]["proofs"][0]
        raw = (self.vendor / proof["path"]).read_bytes() + b"changed"
        (self.vendor / proof["path"]).write_bytes(raw)
        proof.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        self.seal(proof["path"])
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "Historical provenance"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_source_archive_matches_exact_package_and_is_not_only_a_link(self):
        result = stage_overlay(self.vendor, self.destination, self.packages)
        source = result["record"]["source_archives"][0]
        archive = self.destination.parent.parent / source["path"]
        self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), source["sha256"])
        with tarfile.open(archive) as contents:
            self.assertEqual(len(contents.getmembers()), 22)
            original = contents.extractfile("selectors-0.38.0/Cargo.toml.orig").read()
            self.assertEqual(original, (self.vendor / "local-evidence/selectors-0.38.0/Cargo.toml.orig").read_bytes())
        shutil.rmtree(self.destination)
        row = self.manifest["source_archives"][0]
        raw = (self.vendor / row["path"]).read_bytes() + b"changed"
        (self.vendor / row["path"]).write_bytes(raw)
        row.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        self.seal(row["path"])
        self.save_manifest()
        with self.assertRaisesRegex(RuntimeError, "reviewed exact package"):
            stage_overlay(self.vendor, self.destination, self.packages)
        self.assertFalse(self.destination.exists())

    def test_wheel_and_npm_payloads_include_verified_offline_source_and_notices(self):
        files = upstream_files(self.repository)
        source_name = "licenses/upstream/source-archives/selectors-0.38.0.crate"
        self.assertEqual(files[source_name], (self.vendor / "source-archives/selectors-0.38.0.crate").read_bytes())
        source = self.root / "base.whl"
        wheel = self.root / "final.whl"
        with zipfile.ZipFile(source, "w") as archive:
            archive.writestr("markitai/__init__.py", "fixture")
            archive.writestr("markitai-1.dist-info/RECORD", "markitai/__init__.py,,\nmarkitai-1.dist-info/RECORD,,\n")
        supplement_wheel_licenses(source, wheel, files)
        with zipfile.ZipFile(wheel) as archive:
            for name, raw in files.items():
                self.assertEqual(archive.read("markitai-1.dist-info/licenses/" + name), raw)
        npm = self.root / "module.tgz"
        with tarfile.open(npm, "w:gz") as archive:
            for name, raw in files.items():
                member = tarfile.TarInfo("package/" + name)
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
        verify_node_licenses(npm, files)
        # A distributor cannot silently omit the covered-source copy.
        with tarfile.open(npm, "w:gz") as archive:
            for name, raw in files.items():
                if name == source_name:
                    continue
                member = tarfile.TarInfo("package/" + name)
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
        with self.assertRaises(RuntimeError):
            verify_node_licenses(npm, files)


if __name__ == "__main__":
    unittest.main()
