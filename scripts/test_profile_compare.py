"""Counterexamples that must invalidate profile-equivalence evidence."""

from collections import Counter
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from profile_compare import compare


def write_json(path, value):
    path.write_text(json.dumps(value), encoding="utf-8")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


class ProfileEvidenceContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.serial = 0

    def fixture_pair(self):
        """Saved evidence only: no FFI, reference import or conversion is run."""
        self.serial += 1
        root = self.root / str(self.serial)
        sources = root / "inputs"
        sources.mkdir(parents=True)
        source_paths = []
        for index in range(24):
            source = sources / f"case-{index}.txt"
            source.write_bytes(f"fixture {index}".encode())
            source_paths.append(source)
        audits, hashes = [], []
        for profile in ("release", "dist"):
            directory = root / profile
            directory.mkdir()
            library = directory / "fixture.dylib"
            library.write_bytes(f"synthetic {profile} artifact".encode())
            library_hash = hashlib.sha256(library.read_bytes()).hexdigest()
            cases = []
            for index, source in enumerate(source_paths):
                folder = directory / f"{index:02}-{source.name}"
                folder.mkdir()
                write_json(folder / "request.json", {"source": str(source)})
                cases.append({"name": source.name, "input_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                              "status": "output_drift", "matches": {"markdown": False}})
                for engine in ("native", "reference"):
                    assets = []
                    if index == 0:
                        asset_directory = folder / engine / ".markitai" / "assets"
                        asset_directory.mkdir(parents=True)
                        for name, content in (("first.bin", b"a1"), ("second.bin", b"b22")):
                            (asset_directory / name).write_bytes(content)
                            assets.append({"name": name, "bytes": len(content),
                                           "sha256": hashlib.sha256(content).hexdigest()})
                    write_json(folder / f"{engine}-response.json", {
                        "ok": True, "markdown": f"# {engine} body\n", "frontmatter": {
                            "title": "Source title", "markitai_processed": profile},
                        "warnings": [], "skip_reason": None, "assets": assets})
            write_json(directory / "report.json", {
                "schema": 1, "reference_revision": "reference-fixture", "reference_dirty": False,
                "library": str(library), "library_sha256": library_hash,
                "scope": "Synthetic saved evidence", "normalization": ["frontmatter.markitai_processed"],
                "counts": {"output_drift": 24}, "cases": cases})
            audits.append(directory)
            hashes.append(library_hash)
        return audits[0], audits[1], hashes, root

    def run_comparison(self, pair):
        baseline, candidate, hashes, root = pair
        output = root / f"comparison-{len(list(root.glob('comparison-*.json')))}.json"
        return compare("formats", baseline, candidate, hashes[0], hashes[1], output)

    def response_path(self, directory, engine="native"):
        return directory / "00-case-0.txt" / f"{engine}-response.json"

    def test_processing_clock_is_excluded_but_metadata_and_unknown_fields_are_not(self):
        pair = self.fixture_pair()
        self.assertTrue(self.run_comparison(pair)["all_equivalent"])
        for field, old, new, expected_path in (
            ("frontmatter", {"title": "Source title"}, {"title": "Wrong title"}, "/frontmatter/title"),
            ("additional", None, "unexpected", "/additional"),
            ("additional", False, 0, "/additional"),
        ):
            with self.subTest(field=field, old=old, new=new):
                pair = self.fixture_pair()
                for directory, value in ((pair[0], old), (pair[1], new)):
                    path = self.response_path(directory)
                    payload = read_json(path)
                    if value is not None:
                        payload[field] = value
                    write_json(path, payload)
                result = self.run_comparison(pair)
                changed = next(case for case in result["cases"] if not case["equal"])
                self.assertIn(expected_path, changed["native"]["changed_paths"])

    def test_missing_and_mismatched_case_sets_are_rejected(self):
        for mutation in ("missing", "renamed"):
            with self.subTest(mutation=mutation):
                pair = self.fixture_pair()
                path = pair[1] / "report.json"
                report = read_json(path)
                if mutation == "missing":
                    report["cases"].pop()
                    report["counts"] = dict(Counter(case["status"] for case in report["cases"]))
                else:
                    report["cases"][0]["name"] = "other.txt"
                    (pair[1] / "00-case-0.txt").rename(pair[1] / "00-other.txt")
                write_json(path, report)
                with self.assertRaises(AssertionError):
                    self.run_comparison(pair)

    def test_equal_status_counts_do_not_hide_a_changed_body(self):
        pair = self.fixture_pair()
        path = self.response_path(pair[1])
        payload = read_json(path)
        payload["markdown"] = "# Truncated body\n"
        write_json(path, payload)
        result = self.run_comparison(pair)
        self.assertEqual(result["baseline"]["counts"], result["candidate"]["counts"])
        self.assertEqual(result["equivalent_cases"], 23)
        self.assertFalse(result["all_equivalent"])
        changed = next(case for case in result["cases"] if not case["equal"])
        self.assertEqual(changed["case_record_changed_paths"], [])
        self.assertEqual(changed["native"]["changed_paths"], ["/markdown"])
        self.assertTrue(any(pair[3].glob("comparison-*-diffs/*.md.diff")))

    def test_warnings_and_asset_order_are_not_lost_in_summary_counts(self):
        for field in ("warnings", "assets"):
            with self.subTest(field=field):
                pair = self.fixture_pair()
                path = self.response_path(pair[1])
                payload = read_json(path)
                payload[field] = ["Text recovery lost layout"] if field == "warnings" else list(reversed(payload[field]))
                write_json(path, payload)
                result = self.run_comparison(pair)
                self.assertFalse(result["all_equivalent"])
                changed = next(case for case in result["cases"] if not case["equal"])
                self.assertTrue(any(path.startswith("/" + field) for path in changed["native"]["changed_paths"]))
                self.assertTrue(changed["reference"]["equal"])

    def test_equal_error_codes_do_not_hide_changed_error_messages(self):
        pair = self.fixture_pair()
        for directory in pair[:2]:
            path = directory / "report.json"
            report = read_json(path)
            report["cases"][0]["status"] = "native_error"
            report["counts"] = dict(Counter(case["status"] for case in report["cases"]))
            write_json(path, report)
        for directory, message in ((pair[0], "Unsupported input"), (pair[1], "Corrupt input")):
            write_json(self.response_path(directory), {"ok": False, "error": {"code": "conversion_error", "message": message}})
        result = self.run_comparison(pair)
        changed = next(case for case in result["cases"] if not case["equal"])
        self.assertEqual(changed["native"]["changed_paths"], ["/error/message"])

    def test_missing_or_corrupt_assets_cannot_reuse_trusted_recorded_hashes(self):
        for mutation in ("missing", "same_length_corruption", "forged_hash"):
            with self.subTest(mutation=mutation):
                pair = self.fixture_pair()
                asset = pair[1] / "00-case-0.txt" / "native" / ".markitai" / "assets" / "first.bin"
                if mutation == "missing":
                    asset.unlink()
                elif mutation == "same_length_corruption":
                    asset.write_bytes(b"xx")
                else:
                    path = self.response_path(pair[1])
                    payload = read_json(path)
                    payload["assets"][0]["sha256"] = "0" * 64
                    write_json(path, payload)
                with self.assertRaises(AssertionError):
                    self.run_comparison(pair)

    def test_changed_artifact_and_source_bytes_invalidate_evidence(self):
        for mutation in ("library", "expected_library_hash", "source"):
            with self.subTest(mutation=mutation):
                pair = self.fixture_pair()
                if mutation == "library":
                    (pair[1] / "fixture.dylib").write_bytes(b"replacement binary")
                elif mutation == "expected_library_hash":
                    pair[2][1] = "0" * 64
                else:
                    (pair[3] / "inputs" / "case-0.txt").write_bytes(b"changed source")
                with self.assertRaises(AssertionError):
                    self.run_comparison(pair)


if __name__ == "__main__":
    unittest.main()
