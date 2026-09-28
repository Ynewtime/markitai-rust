"""Acceptance tests for the migration audit's failure accounting."""

import tempfile
import unittest
from pathlib import Path

from audit_formats import compare


class AuditContracts(unittest.TestCase):
    def result(self, **changes):
        return {"ok": True, "markdown": "# Title\n\nBody", "frontmatter": {
            "title": "Title", "markitai_processed": "first-clock"}, "assets": [],
            "skip_reason": None, **changes}

    def check(self, before, after):
        with tempfile.TemporaryDirectory() as path:
            return compare(before, after, Path(path))

    def test_clock_is_the_only_metadata_normalization(self):
        after = self.result(frontmatter={"title": "Title", "markitai_processed": "later-clock"})
        self.assertEqual(self.check(self.result(), after)["status"], "parity_pass")
        after["frontmatter"]["title"] = "Missing original title"
        self.assertEqual(self.check(self.result(), after)["status"], "output_drift")

    def test_equal_text_does_not_hide_lost_assets(self):
        before = self.result(assets=[{"sha256": "original"}])
        after = self.result(assets=[{"sha256": "different"}])
        result = self.check(before, after)
        self.assertTrue(result["matches"]["markdown"])
        self.assertFalse(result["matches"]["asset_hashes"])
        self.assertEqual(result["status"], "output_drift")

    def test_unsupported_and_reference_failure_are_never_successes(self):
        error = {"ok": False, "error": {"code": "unsupported", "message": "Not yet implemented"}}
        self.assertEqual(self.check(self.result(), error)["status"], "unsupported")
        self.assertEqual(self.check(error, self.result())["status"], "reference_error")

    def test_word_recall_does_not_hide_structure_loss(self):
        result = self.check(self.result(), self.result(markdown="Title Body"))
        self.assertEqual(result["reference_token_recall"], 1.0)
        self.assertEqual(result["status"], "output_drift")


if __name__ == "__main__":
    unittest.main()
