"""Exact attribution transport and single-executable archive counterexamples; no builds."""
from pathlib import Path
import base64
import csv
import hashlib
import io
import json
import os
import tarfile
import tempfile
import unittest
import zipfile

from codex_attribution import COMPILED, NAMES, codex_files, stage_codex_files
from ci_packages import (extract_single_binary_tar, supplement_wheel_licenses,
                         verify_node_licenses, write_single_binary_tar)


class CodexAttributionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        directory = self.source / "licenses/codex"
        directory.mkdir(parents=True)
        notice = b"Authored license transport fixture\r\n"
        catalog = b'{"models":[{"slug":"gpt-5.5","apply_patch_tool_type":null}]}\n'
        (directory / "LICENSE").write_bytes(notice)
        (directory / "NOTICE").write_bytes(b"Authored upstream NOTICE\n")
        (directory / "models.json").write_bytes(catalog)
        (directory / "README.md").write_bytes(b"Authored modification notice\n")
        (directory / "provenance.json").write_text(json.dumps({
            "schema_version": 1, "license_file": "LICENSE", "notice_file": "NOTICE", "compiled_catalog": COMPILED,
            "notice_sha256": hashlib.sha256(b"Authored upstream NOTICE\n").hexdigest(),
            "license_sha256": hashlib.sha256(notice).hexdigest(),
            "catalog_sha256": hashlib.sha256(catalog).hexdigest(), "models": ["gpt-5.5"],
        }))
        compiled = self.source / COMPILED
        compiled.parent.mkdir(parents=True)
        compiled.write_bytes(catalog)
        self.directory = directory

    def test_go_and_native_staging_preserve_complete_attribution_and_refuse_overwrite(self):
        expected = codex_files(self.source)
        for kind in ["go", "native"]:
            target = self.root / kind
            record = stage_codex_files(self.source, target)
            self.assertEqual(set(record), set(expected))
            for name, content in expected.items():
                self.assertEqual((target / name).read_bytes(), content)
                self.assertEqual(record[name]["sha256"], hashlib.sha256(content).hexdigest())
            with self.assertRaises(FileExistsError):
                stage_codex_files(self.source, target)

    def test_source_license_and_compiled_catalog_changes_cannot_silently_ship(self):
        for path in [self.directory / "LICENSE", self.directory / "NOTICE", self.directory / "models.json", self.source / COMPILED]:
            old = path.read_bytes()
            path.write_bytes(old + b"changed")
            with self.assertRaises(RuntimeError):
                codex_files(self.source)
            path.write_bytes(old)
        self.assertEqual(set(codex_files(self.source)), {"licenses/codex/" + name for name in NAMES})

    @unittest.skipIf(os.name == "nt", "symlink privileges are host-dependent")
    def test_attribution_symlink_cannot_redirect_the_pinned_notice(self):
        path = self.directory / "README.md"
        outside = self.root / "outside"
        outside.write_bytes(path.read_bytes())
        path.unlink()
        path.symlink_to(outside)
        with self.assertRaisesRegex(RuntimeError, "regular"):
            codex_files(self.source)

    def test_node_requires_all_exact_nested_codex_files(self):
        expected = codex_files(self.source)
        for mode in ["valid", "missing", "wrong", "duplicate"]:
            package = self.root / (mode + ".tgz")
            with tarfile.open(package, "x:gz") as archive:
                for i, (name, content) in enumerate(expected.items()):
                    if i == 0 and mode == "missing":
                        continue
                    value = content + b"wrong" if i == 0 and mode == "wrong" else content
                    for _ in range(2 if i == 0 and mode == "duplicate" else 1):
                        member = tarfile.TarInfo("package/" + name)
                        member.size = len(value)
                        archive.addfile(member, io.BytesIO(value))
            if mode == "valid":
                verify_node_licenses(package, expected)
            else:
                with self.assertRaises(RuntimeError):
                    verify_node_licenses(package, expected)

    def test_wheel_records_complete_codex_notice_and_exact_data_hashes(self):
        source = self.root / "raw.whl"
        target = self.root / "final.whl"
        expected = codex_files(self.source)
        with zipfile.ZipFile(source, "x") as archive:
            archive.writestr("markitai/_native.so", b"authored-native-bytes")
            archive.writestr("markitai-1.dist-info/RECORD", "")
        supplement_wheel_licenses(source, target, expected)
        with zipfile.ZipFile(target) as archive:
            rows = {r[0]: r[1:] for r in csv.reader(io.StringIO(archive.read("markitai-1.dist-info/RECORD").decode()))}
            self.assertEqual(archive.read("markitai/_native.so"), b"authored-native-bytes")
            for name, content in expected.items():
                path = "markitai-1.dist-info/licenses/" + name
                self.assertEqual(archive.read(path), content)
                digest = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).rstrip(b"=").decode()
                self.assertEqual(rows[path], ["sha256=" + digest, str(len(content))])

    @unittest.skipIf(os.name == "nt", "Unix distribution uses a real relative symlink")
    def test_single_binary_tar_has_one_executable_and_verified_alias_and_all_notices(self):
        binary = self.root / "source-binary"
        binary.write_bytes(bytes(range(256)) * 4)
        expected = codex_files(self.source)
        expected["NOTICE"] = b"All required license locations\n"
        archive = self.root / "cli.tar.gz"
        write_single_binary_tar(binary, archive, expected)
        destination = self.root / "unpacked"
        record = extract_single_binary_tar(archive, destination, binary, expected)
        with tarfile.open(archive) as bundle:
            self.assertEqual([m.name for m in bundle.getmembers() if m.isfile() and m.mode & 0o111], ["markitai"])
            self.assertTrue(bundle.getmember("mkai").issym())
            self.assertTrue(bundle.getmember("markitai-mcp").issym())
        self.assertEqual(os.readlink(destination / "mkai"), "markitai")
        self.assertEqual(os.readlink(destination / "markitai-mcp"), "markitai")
        self.assertEqual((destination / "markitai-mcp").read_bytes(), binary.read_bytes())
        self.assertEqual((destination / "mkai").read_bytes(), binary.read_bytes())
        self.assertEqual(record["executable"]["bytes"], binary.stat().st_size)
        for name, content in expected.items():
            self.assertEqual((destination / name).read_bytes(), content)
        with self.assertRaises(FileExistsError):
            write_single_binary_tar(binary, archive, expected)

    @unittest.skipIf(os.name == "nt", "Unix archive contract")
    def test_invalid_archive_inventory_alias_and_notice_are_rejected_before_extract(self):
        binary = self.root / "source-binary"
        binary.write_bytes(b"fixture binary")
        expected = {"licenses/codex/LICENSE": b"complete notice"}
        for mode in ["duplicate", "extra", "alias", "mcp-alias", "notice", "hardlink"]:
            archive = self.root / (mode + ".tar.gz")
            with tarfile.open(archive, "x:gz") as bundle:
                for name, content in [("markitai", binary.read_bytes()), *expected.items()]:
                    if mode == "notice" and name in expected:
                        content = b"different"
                    member = tarfile.TarInfo(name)
                    member.mode = 0o755 if name == "markitai" else 0o644
                    member.size = len(content)
                    bundle.addfile(member, io.BytesIO(content))
                    if mode == "duplicate" and name == "markitai":
                        bundle.addfile(member, io.BytesIO(content))
                alias = tarfile.TarInfo("mkai")
                alias.type = tarfile.LNKTYPE if mode == "hardlink" else tarfile.SYMTYPE
                alias.linkname = "../outside" if mode == "alias" else "markitai"
                bundle.addfile(alias)
                mcp = tarfile.TarInfo("markitai-mcp")
                mcp.type = tarfile.SYMTYPE
                mcp.linkname = "../outside" if mode == "mcp-alias" else "markitai"
                bundle.addfile(mcp)
                if mode == "extra":
                    bundle.addfile(tarfile.TarInfo("unexpected"))
            target = self.root / ("out-" + mode)
            with self.assertRaises(RuntimeError):
                extract_single_binary_tar(archive, target, binary, expected)
            self.assertFalse(target.exists())

    def test_unsafe_or_binary_colliding_attribution_names_are_rejected(self):
        binary = self.root / "source-binary"
        binary.write_bytes(b"fixture")
        for name in ["../bad", "/bad", "a/../bad", "a//bad", "a\\bad", "markitai", "mkai/nested", "markitai-mcp", "markitai-mcp/nested"]:
            with self.assertRaises(RuntimeError):
                write_single_binary_tar(binary, self.root / "bad.tar.gz", {name: b"data"})


if __name__ == "__main__":
    unittest.main()
