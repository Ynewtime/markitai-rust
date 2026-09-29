"""Static-package counterexamples; no native compiler, conversion or installation."""
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from package_go_static import (bundle_licenses, dependency_closure, identity,
                              inventory, package_archive, parse_native_flags,
                              unpack_verified, validate_linkage, verify_build)


class StaticGoPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_compiler_note_keeps_only_explicit_system_dependencies(self):
        raw = "Compiling markitai\nnote: native-static-libs: -framework Vision -lobjc -lc -lc\nFinished\n"
        self.assertEqual(parse_native_flags(raw), [("framework", "Vision"), ("library", "c"), ("library", "objc")])
        for bad in ["Finished", "native-static-libs: ", "native-static-libs: -L/private -lfoo",
                    "native-static-libs: -Wl,-rpath,/repo", "native-static-libs: -framework ../bad",
                    "native-static-libs: -lc\nnative-static-libs: -lm"]:
            with self.subTest(raw=bad), self.assertRaises(RuntimeError):
                parse_native_flags(bad)

    def test_static_linkage_cannot_fall_back_to_a_dynamic_library(self):
        required = parse_native_flags("native-static-libs: -framework Vision -lobjc")
        prefix = "#cgo LDFLAGS: ${SRCDIR}/native/darwin_arm64/libmarkitai_ffi.a "
        self.assertIn(("framework", "Vision"), validate_linkage(prefix + "-framework Vision -lobjc -lc", required))
        for wrong in ["#cgo LDFLAGS: -L${SRCDIR}/native -lmarkitai_ffi -framework Vision -lobjc",
                      prefix + "-lobjc", prefix + "-framework Vision -lobjc -L/tmp"]:
            with self.assertRaises(RuntimeError):
                validate_linkage(wrong, required)

    def metadata(self):
        repository = self.root / "repository"
        source = repository / "crates/markitai-ffi"
        source.mkdir(parents=True)
        (repository / "LICENSE").write_bytes(b"root license\n")
        (repository / "NOTICE").write_bytes(b"root attribution\n")
        package = self.root / "registry/dependency"
        package.mkdir(parents=True)
        (package / "LICENSE-MIT").write_bytes(b"first copyright\r\nMIT text\r\n")
        (package / "LICENSE-other-bits").write_bytes(b"bundled original terms\n")
        (package / "terms.txt").write_bytes(b"declared license file\n")
        (package / "license.rs").write_text("// This is code, not license evidence")
        missing = self.root / "registry/missing"
        missing.mkdir()
        packages = [
            {"id": "ffi", "name": "markitai-ffi", "version": "1", "source": None,
             "license": "MIT", "license_file": None, "manifest_path": str(source / "Cargo.toml")},
            {"id": "dep", "name": "dependency", "version": "2", "source": "registry+fixture",
             "license": "MIT", "license_file": "terms.txt", "manifest_path": str(package / "Cargo.toml")},
            {"id": "missing", "name": "missing", "version": "3", "source": "registry+fixture",
             "license": "MIT", "license_file": None, "manifest_path": str(missing / "Cargo.toml")},
            {"id": "unrelated", "name": "not-linked", "version": "4", "source": "registry+fixture"},
        ]
        metadata = {"packages": packages, "resolve": {"nodes": [
            {"id": "ffi", "deps": [{"pkg": "dep"}, {"pkg": "missing"}]},
            {"id": "dep", "deps": [{"pkg": "missing"}]}, {"id": "missing", "deps": []}]}}
        return repository, metadata

    def test_license_collection_preserves_texts_and_exposes_missing_sources(self):
        repository, metadata = self.metadata()
        sysroot = self.root / "rust"
        text = sysroot / "share/doc/rust/COPYRIGHT-library.html"
        text.parent.mkdir(parents=True)
        text.write_bytes(b"<p>Toolchain original notice</p>\n")
        (text.parent / "licenses").mkdir()
        (text.parent / "licenses/MIT.txt").write_bytes(b"Original toolchain MIT text\n")
        stage = self.root / "module"
        stage.mkdir()
        record = bundle_licenses(metadata, stage / "licenses", repository, sysroot)
        self.assertEqual({item["id"] for item in record["dependencies"]}, {"ffi", "dep", "missing"})
        self.assertEqual(record["legal_review"], "not_performed")
        self.assertEqual([item["id"] for item in record["unresolved"]], ["missing"])
        dependency = next(item for item in record["dependencies"] if item["id"] == "dep")
        self.assertEqual(len(dependency["texts"]), 3)
        for item in dependency["texts"] + record["rust_toolchain_texts"]:
            self.assertEqual((stage / item["path"]).read_bytes(), Path(item["source"]).read_bytes())
            self.assertEqual(identity(stage / item["path"])["sha256"], item["sha256"])
        self.assertEqual(len(record["rust_toolchain_texts"]), 2)
        metadata["resolve"]["nodes"].pop()
        with self.assertRaisesRegex(RuntimeError, "Incomplete"):
            dependency_closure(metadata)

    def test_declared_notice_cannot_escape_its_package(self):
        repository, metadata = self.metadata()
        metadata["packages"][1]["license_file"] = "../../outside.txt"
        with self.assertRaisesRegex(RuntimeError, "escapes"):
            bundle_licenses(metadata, self.root / "licenses", repository, self.root / "rust")

    def test_archive_is_relocatable_exact_and_rejects_missing_or_changed_members(self):
        module = self.root / "module"
        (module / "native/include").mkdir(parents=True)
        (module / "markitai.go").write_bytes(b"package markitai\n")
        (module / "native/include/markitai.h").write_bytes(bytes(range(256)))
        expected = inventory(module)
        archive = self.root / "package.tgz"
        self.assertEqual(package_archive(module, archive), expected)
        installed = self.root / "installed"
        unpack_verified(archive, installed, expected)
        self.assertEqual(inventory(installed), expected)
        second = self.root / "second.tgz"
        package_archive(module, second)
        self.assertEqual(archive.read_bytes(), second.read_bytes())
        with self.assertRaises(FileExistsError):
            unpack_verified(archive, installed, expected)
        wrong = {**expected, "missing.go": identity(module / "markitai.go")}
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            unpack_verified(archive, self.root / "missing", wrong)
        (module / "markitai.go").write_bytes(b"changed payload\n")
        package_archive(module, self.root / "changed.tgz")
        with self.assertRaisesRegex(RuntimeError, "differs"):
            unpack_verified(self.root / "changed.tgz", self.root / "changed", expected)

    def test_archive_links_and_duplicates_are_not_installable(self):
        for kind in ["symlink", "duplicate"]:
            archive = self.root / f"{kind}.tgz"
            expected = {"file": {"bytes": 1, "sha256": hashlib.sha256(b"x").hexdigest()}}
            with tarfile.open(archive, "w:gz") as bundle:
                entry = tarfile.TarInfo("markitai-go/file")
                if kind == "symlink":
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = "../../outside"
                    bundle.addfile(entry)
                else:
                    entry.size = 1
                    bundle.addfile(entry, io.BytesIO(b"x"))
                    bundle.addfile(entry, io.BytesIO(b"x"))
            with self.assertRaises(RuntimeError):
                unpack_verified(archive, self.root / kind, expected)

    def test_build_identity_checks_bytes_not_only_a_supplied_revision(self):
        source = self.root / "file.rs"
        source.write_bytes(b"original")
        library = self.root / "library.a"
        library.write_bytes(b"archive")
        build = {"source_revision": "a" * 40, "status": "passed", "source_unchanged": True,
                 "source_files": {"file.rs": identity(source)}, "archive": identity(library)}
        def git(command, **kwargs):
            return {"rev-parse": "a" * 40, "status": b"", "ls-files": b"file.rs\0"}[command[1]]
        with patch("package_go_static.subprocess.check_output", side_effect=git):
            self.assertEqual(verify_build(self.root, "a" * 40, build, {"archive": library}), build["source_files"])
            source.write_bytes(b"modified")
            with self.assertRaisesRegex(RuntimeError, "Source files"):
                verify_build(self.root, "a" * 40, build, {"archive": library})
            source.write_bytes(b"original")
            library.write_bytes(b"changed")
            with self.assertRaisesRegex(RuntimeError, "archive differs"):
                verify_build(self.root, "a" * 40, build, {"archive": library})


if __name__ == "__main__":
    unittest.main()
