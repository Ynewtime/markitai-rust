"""Guard against false equivalence in complete native response comparisons."""

from copy import deepcopy
import unittest

from compare_full_envelopes import differing_paths, normalize


class ProfileEnvelopeContracts(unittest.TestCase):
    def envelope(self):
        return {
            "ok": True,
            "result": {
                "source": "fixture.html",
                "markdown": "# Title\n\nBody",
                "duration": 0.25,
                "frontmatter": {"title": "Title", "markitai_processed": "first-clock"},
                "assets": ["one.png", "two.png"],
                "warnings": ["first", "second"],
                "images": [{"duration": 8, "markitai_processed": "image-data"}],
                "usage": {"requests": 0, "cost_usd": 0.0},
                "output_path": None,
            },
        }

    def changes(self, before, after):
        return differing_paths(normalize(before), normalize(after))

    def test_only_two_confirmed_clock_fields_are_removed_without_mutation(self):
        before = self.envelope()
        untouched = deepcopy(before)
        after = deepcopy(before)
        after["result"]["duration"] = 3.75
        after["result"]["frontmatter"]["markitai_processed"] = "later-clock"
        self.assertEqual(self.changes(before, after), [])
        self.assertEqual(before, untouched)
        after["result"]["frontmatter"]["title"] = "Different title"
        self.assertEqual(self.changes(before, after), ["$.result.frontmatter.title"])

    def test_similarly_named_nested_fields_remain_significant(self):
        before = self.envelope()
        after = deepcopy(before)
        after["result"]["images"][0]["duration"] = 9
        after["result"]["images"][0]["markitai_processed"] = "changed-data"
        self.assertEqual(self.changes(before, after), [
            "$.result.images[0].duration", "$.result.images[0].markitai_processed",
        ])

    def test_json_types_are_not_equal_merely_because_python_values_compare_equal(self):
        for left, right in [(0, False), (1, True), (1, 1.0), ([], {}), (None, "")]:
            with self.subTest(left=left, right=right):
                self.assertEqual(differing_paths({"value": left}, {"value": right}), ["$.value"])

    def test_array_order_length_and_null_are_preserved(self):
        before = self.envelope()
        after = deepcopy(before)
        after["result"]["assets"].reverse()
        after["result"]["warnings"].reverse()
        self.assertEqual(self.changes(before, after), [
            "$.result.assets[0]", "$.result.assets[1]",
            "$.result.warnings[0]", "$.result.warnings[1]",
        ])
        after = deepcopy(before)
        after["result"]["assets"].pop()
        after["result"]["warnings"] = None
        self.assertEqual(self.changes(before, after), ["$.result.assets", "$.result.warnings"])

    def test_unknown_fields_and_field_removal_are_detected(self):
        before = self.envelope()
        after = deepcopy(before)
        after["future_envelope"] = {"revision": 2}
        after["result"]["future_result"] = ["preserve me"]
        del after["result"]["output_path"]
        self.assertEqual(self.changes(before, after), [
            "$.future_envelope", "$.result.future_result", "$.result.output_path",
        ])
        self.assertEqual(self.changes(after, before), self.changes(before, after))

    def test_error_code_message_and_success_transition_are_detected(self):
        before = {"ok": False, "error": {"code": "unsupported", "message": "Original"}}
        self.assertEqual(self.changes(before, deepcopy(before)), [])
        after = {"ok": False, "error": {"code": "conversion_error", "message": "Changed"}}
        self.assertEqual(self.changes(before, after), ["$.error.code", "$.error.message"])
        self.assertEqual(self.changes(before, self.envelope()), ["$.error", "$.ok", "$.result"])

    def test_text_boundaries_and_unicode_are_never_normalized(self):
        before = self.envelope()
        for markdown in ["# Title\nBody", "# Title\n\n Body", "# Title\n\nBody\n", "# Title\n\nBo\u200bdy", "# Title\n\n正文"]:
            with self.subTest(markdown=markdown):
                after = deepcopy(before)
                after["result"]["markdown"] = markdown
                self.assertEqual(self.changes(before, after), ["$.result.markdown"])


if __name__ == "__main__":
    unittest.main()
