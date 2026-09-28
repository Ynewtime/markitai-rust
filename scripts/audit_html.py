#!/usr/bin/env python3
"""Audit local HTML API parity; report upstream extraction quality separately."""

from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import difflib
import fnmatch
import json
from pathlib import Path
import shutil
import subprocess
import sys

from audit_formats import comparable_metadata, digest, run_worker


def expected_document(path: Path) -> tuple[dict, str]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    if lines and lines[0].strip() == "```json":
        for index, line in enumerate(lines[1:], 1):
            if line.strip() == "```":
                metadata = json.loads("".join(lines[1:index]))
                if not isinstance(metadata, dict):
                    raise ValueError(f"Expected metadata object in {path}")
                return metadata, "".join(lines[index + 1:]).strip()
        raise ValueError(f"Unclosed metadata block in {path}")
    return {}, text.strip()


def word_count(text: str) -> int:
    # Match the corpus's approximate Unicode-aware word-count diagnostic.
    total = 0
    pending = []
    for character in text:
        codepoint = ord(character)
        if any(low <= codepoint <= high for low, high in (
            (0x4E00, 0x9FFF), (0x3400, 0x4DBF), (0x20000, 0x2A6DF),
            (0x2A700, 0x2B73F), (0x2B740, 0x2B81F), (0xF900, 0xFAFF),
            (0x3000, 0x303F), (0x3040, 0x309F), (0x30A0, 0x30FF),
            (0xAC00, 0xD7AF),
        )):
            total += len("".join(pending).split()) + 1
            pending.clear()
        else:
            pending.append(character)
    return total + len("".join(pending).split())


def quality(result: dict, metadata: dict, body: str) -> dict:
    if not result.get("ok"):
        return {"available": False}
    markdown = result["markdown"]
    expected_words = word_count(body)
    actual_words = word_count(markdown)
    ratio = actual_words / expected_words if expected_words else None
    noise = [phrase for phrase in (
        "Sign up", "Log in", "Cookie", "Accept all cookies", "Privacy Policy",
        "Terms of Service", "Don't miss what's happening", "Something went wrong",
        "Retry", "New to X?",
    ) if phrase in markdown and phrase not in body]
    title = metadata.get("title")
    return {
        "available": True,
        "nonempty": bool(markdown.strip()),
        "title_present_when_expected": bool(result["frontmatter"].get("title")) if title else None,
        "title_equal": result["frontmatter"].get("title") == title if title else None,
        "body_equal_after_outer_trim": markdown.strip() == body,
        "actual_words": actual_words,
        "expected_words": expected_words,
        "word_ratio": round(ratio, 4) if ratio is not None else None,
        "word_ratio_within_0_5_to_2": 0.5 <= ratio <= 2 if ratio is not None else None,
        "extra_chrome_phrases": noise,
    }


def compare(before: dict, after: dict, directory: Path) -> dict:
    if not before.get("ok") or not after.get("ok"):
        return {
            "status": "both_error" if not before.get("ok") and not after.get("ok")
            else "reference_error" if not before.get("ok") else "native_error",
            "reference_error": before.get("error"), "native_error": after.get("error"),
        }
    matches = {
        "markdown": before["markdown"] == after["markdown"],
        "frontmatter": comparable_metadata(before["frontmatter"]) == comparable_metadata(after["frontmatter"]),
        "assets": sorted((item["name"], item["sha256"]) for item in before["assets"])
        == sorted((item["name"], item["sha256"]) for item in after["assets"]),
        "warnings": before.get("warnings", []) == after.get("warnings", []),
        "skip_reason": before.get("skip_reason") == after.get("skip_reason"),
    }
    for key in ("markdown", "frontmatter"):
        if matches[key]:
            continue
        old, new = before[key], after[key]
        if key == "frontmatter":
            old = json.dumps(comparable_metadata(old), ensure_ascii=False, indent=2, sort_keys=True)
            new = json.dumps(comparable_metadata(new), ensure_ascii=False, indent=2, sort_keys=True)
        difference = difflib.unified_diff(old.splitlines(True), new.splitlines(True),
                                          fromfile="reference", tofile="native")
        (directory / f"{key}.diff").write_text("".join(difference), encoding="utf-8")
    return {"status": "parity_pass" if all(matches.values()) else "output_drift", "matches": matches}


def revision(path: Path) -> dict:
    def git(*arguments: str) -> str:
        result = subprocess.run(["git", *arguments], cwd=path, capture_output=True,
                                text=True, check=True)
        return result.stdout.strip()
    return {"head": git("rev-parse", "HEAD"), "dirty": bool(git("status", "--porcelain"))}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--reference-python", type=Path)
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pattern", action="append", help="Select fixture stems with shell patterns; repeatable")
    parser.add_argument("--limit", type=int, help="Explicit subset for smoke checks; never reported as the full corpus")
    parser.add_argument("--jobs", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--require-parity", action="store_true")
    args = parser.parse_args()
    reference, library, output = args.reference.resolve(), args.library.resolve(), args.output.resolve()
    python = (args.reference_python or reference / ".venv/bin/python").absolute()
    if args.jobs < 1 or args.timeout < 1 or (args.limit is not None and args.limit < 1):
        parser.error("--jobs, --timeout and --limit must be positive")
    if output == reference or reference in output.parents:
        parser.error("Output must be outside the read-only reference repository")
    if not library.is_file() or not python.is_file():
        parser.error("Both the native library and reference Python must exist")
    if output.exists() and any(output.iterdir()):
        parser.error("Output directory must be empty")
    corpus = reference / "packages/markitai/tests/defuddle_fixtures"
    discovered = sorted((corpus / "fixtures").glob("*.html"))
    paired = [path for path in discovered if (corpus / "expected" / f"{path.stem}.md").is_file()]
    selected = [path for path in paired if not args.pattern or any(fnmatch.fnmatchcase(path.stem, pattern) for pattern in args.pattern)]
    if args.limit:
        selected = selected[:args.limit]
    if not selected:
        parser.error("No paired HTML fixtures selected")
    output.mkdir(parents=True, exist_ok=True)
    frozen = output / library.name
    shutil.copy2(library, frozen)
    source_state = revision(reference)
    native_state = revision(Path(__file__).resolve().parents[1])

    def audit(item: tuple[int, Path]) -> dict:
        index, source = item
        expected_path = corpus / "expected" / f"{source.stem}.md"
        metadata, body = expected_document(expected_path)
        directory = output / f"{index:03}-{source.stem}"
        directory.mkdir()
        input_hash, expected_hash = digest(source), digest(expected_path)
        request = {"source": str(source), "case_dir": str(directory), "reference": str(reference), "library": str(frozen)}
        request_path = directory / "request.json"
        request_path.write_text(json.dumps(request), encoding="utf-8")
        before = run_worker(str(python), "reference", request_path, args.timeout)
        after = run_worker(sys.executable, "native", request_path, args.timeout)
        if digest(source) != input_hash or digest(expected_path) != expected_hash:
            raise RuntimeError(f"Reference fixture changed during the audit: {source}")
        result = {"name": source.stem, "input_sha256": input_hash, "expected_sha256": expected_hash,
                  **compare(before, after, directory),
                  "upstream_diagnostic": {"reference": quality(before, metadata, body),
                                           "native": quality(after, metadata, body)}}
        print(f"{source.stem}: {result['status']}", flush=True)
        return result

    with ThreadPoolExecutor(max_workers=args.jobs) as executor:
        cases = list(executor.map(audit, enumerate(selected)))
    counts = dict(Counter(case["status"] for case in cases))
    category_counts = {category: dict(Counter(case["status"] for case in cases if case["name"].split("--", 1)[0] == category))
                       for category in sorted({case["name"].split("--", 1)[0] for case in cases})}
    quality_counts = {}
    for engine in ("reference", "native"):
        items = [case["upstream_diagnostic"][engine] for case in cases]
        quality_counts[engine] = {
            "available": sum(item["available"] for item in items),
            "body_equal_after_outer_trim": sum(item.get("body_equal_after_outer_trim", False) for item in items),
            "word_ratio_within_0_5_to_2": sum(item.get("word_ratio_within_0_5_to_2") is True for item in items),
            "title_equal": sum(item.get("title_equal") is True for item in items),
            "extra_chrome_phrases": sum(bool(item.get("extra_chrome_phrases")) for item in items),
        }
    report = {
        "schema": 1, "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "scope": "Local HTML file public API comparison, with model/OCR/browser features disabled. This is not URL extraction or a full port of upstream tests.",
        "normalization": ["frontmatter.markitai_processed only"],
        "strict_pass": "Exact Markdown, frontmatter after timestamp removal, asset names/hashes, warnings and skip reason; matching errors are not passes.",
        "upstream_diagnostic_scope": "Local file outputs versus URL-oriented defuddle expectations. Body outer trim, title equality, word ratio and chrome phrases are diagnostics only; none can turn output drift into a parity pass.",
        "reference": {"path": str(reference), "python": str(python), **source_state},
        "native": {"library": str(library), "frozen_library": str(frozen), "sha256": digest(frozen), **native_state},
        "discovery": {"html_files": len(discovered), "paired_expected": len(paired), "selected": len(selected),
                      "full_paired_corpus": len(selected) == len(paired), "patterns": args.pattern, "limit": args.limit},
        "counts": counts, "category_counts": category_counts,
        "upstream_diagnostic_counts": quality_counts, "cases": cases,
    }
    path = output / "report.json"
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"report": str(path), "counts": counts, "selected": len(cases)}, indent=2))
    return int(args.require_parity and any(case["status"] != "parity_pass" for case in cases))


if __name__ == "__main__":
    raise SystemExit(main())
