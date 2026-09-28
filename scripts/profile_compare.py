#!/usr/bin/env python3
"""Compare full saved API evidence between native build profiles, without conversion."""
from __future__ import annotations

import argparse
from collections import Counter
from copy import deepcopy
import datetime as dt
import difflib
import hashlib
import json
from pathlib import Path


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def canonical(value) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True)


def response(value: dict) -> dict:
    result = deepcopy(value)
    metadata = result.get("frontmatter")
    if isinstance(metadata, dict):
        metadata.pop("markitai_processed", None)
    return result


def changed_paths(before, after, prefix="") -> list[str]:
    if type(before) is not type(after):
        return [prefix or "/"]
    if isinstance(before, dict):
        paths = []
        for key in sorted(set(before) | set(after)):
            path = prefix + "/" + key.replace("~", "~0").replace("/", "~1")
            paths.extend([path] if key not in before or key not in after
                         else changed_paths(before[key], after[key], path))
        return paths
    if isinstance(before, list):
        if len(before) != len(after):
            return [prefix]
        return [path for index, (old, new) in enumerate(zip(before, after))
                for path in changed_paths(old, new, f"{prefix}/{index}")]
    return [] if before == after else [prefix or "/"]


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def read_audit(directory: Path, kind: str, expected_sha: str):
    report_path = directory / "report.json"
    report = load(report_path)
    count = 24 if kind == "formats" else 209
    assert report["schema"] == 1 and len(report["cases"]) == count, "wrong corpus/schema"
    assert Counter(case["status"] for case in report["cases"]) == report["counts"]
    assert len({case["name"] for case in report["cases"]}) == count, "duplicate fixture names"
    if kind == "html":
        assert report["discovery"]["full_paired_corpus"] and report["discovery"]["selected"] == count
        library = directory / Path(report["native"]["frozen_library"]).name
        declared_sha = report["native"]["sha256"]
        reference = (report["reference"]["head"], report["reference"]["dirty"])
    else:
        library = directory / Path(report["library"]).name
        declared_sha = report["library_sha256"]
        reference = (report["reference_revision"], report["reference_dirty"])
    assert not reference[1], "reference was dirty at audit time"
    assert digest(library) == declared_sha == expected_sha, "frozen artifact identity mismatch"
    cases = {}
    asset_files = 0
    for index, case in enumerate(report["cases"]):
        name = case["name"]
        folder = directory / (f"{index:02}-{Path(name).name}" if kind == "formats" else f"{index:03}-{name}")
        request = load(folder / "request.json")
        source = Path(request["source"])
        assert digest(source) == case["input_sha256"], f"input changed: {name}"
        if kind == "html":
            expected = source.parent.parent / "expected" / (source.stem + ".md")
            assert digest(expected) == case["expected_sha256"], f"expectation changed: {name}"
        engines = {}
        for engine in ("native", "reference"):
            payload = load(folder / f"{engine}-response.json")
            assert type(payload.get("ok")) is bool, f"invalid response: {name}/{engine}"
            for asset in payload.get("assets", []):
                assert Path(asset["name"]).name == asset["name"], "asset label is not a basename"
                paths = [path for path in (folder / engine).rglob("*")
                         if path.is_file() and path.name == asset["name"]]
                assert len(paths) == 1, f"ambiguous/missing asset: {name}/{engine}/{asset['name']}"
                assert paths[0].stat().st_size == asset["bytes"]
                assert digest(paths[0]) == asset["sha256"], f"asset changed: {name}/{engine}"
                asset_files += 1
            engines[engine] = response(payload)
        cases[name] = {"record": case, "responses": engines, "source": str(source)}
    identity = {"directory": str(directory), "report_sha256": digest(report_path),
                "library_sha256": declared_sha, "reference_revision": reference[0],
                "counts": report["counts"], "rechecked_asset_files": asset_files}
    return report, cases, identity


def compare(kind: str, baseline: Path, candidate: Path, baseline_sha: str,
            candidate_sha: str, output: Path) -> dict:
    assert not output.exists(), "use a new report path"
    old_report, old, old_identity = read_audit(baseline, kind, baseline_sha)
    new_report, new, new_identity = read_audit(candidate, kind, candidate_sha)
    assert set(old) == set(new), "fixture sets differ"
    assert old_identity["reference_revision"] == new_identity["reference_revision"]
    for key in ("schema", "scope", "normalization", "strict_pass", "upstream_diagnostic_scope"):
        assert old_report.get(key) == new_report.get(key), f"audit method changed: {key}"
    records = []
    diffs = output.with_suffix("").with_name(output.stem + "-diffs")
    for name in sorted(old):
        before, after = old[name], new[name]
        assert before["source"] == after["source"], f"source path changed: {name}"
        for key in ("input_sha256", "expected_sha256"):
            assert before["record"].get(key) == after["record"].get(key), f"fixture hash changed: {name}"
        item = {"name": name, "case_record_changed_paths": changed_paths(before["record"], after["record"])}
        for engine in ("native", "reference"):
            a, b = before["responses"][engine], after["responses"][engine]
            paths = changed_paths(a, b)
            item[engine] = {"equal": not paths, "changed_paths": paths,
                            "baseline_sha256": hashlib.sha256(canonical(a).encode()).hexdigest(),
                            "candidate_sha256": hashlib.sha256(canonical(b).encode()).hexdigest()}
            if paths:
                diffs.mkdir(parents=True, exist_ok=True)
                label = name.replace("/", "__")
                text = "".join(difflib.unified_diff(canonical(a).splitlines(True), canonical(b).splitlines(True),
                                                  fromfile="baseline", tofile="candidate"))
                (diffs / f"{label}.{engine}.json.diff").write_text(text, encoding="utf-8")
                if a.get("markdown") != b.get("markdown"):
                    text = "".join(difflib.unified_diff(a.get("markdown", "").splitlines(True), b.get("markdown", "").splitlines(True),
                                                      fromfile="baseline.md", tofile="candidate.md"))
                    (diffs / f"{label}.{engine}.md.diff").write_text(text, encoding="utf-8")
        item["equal"] = not item["case_record_changed_paths"] and all(item[engine]["equal"] for engine in ("native", "reference"))
        records.append(item)
    result = {"schema": 1, "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
              "scope": f"{kind}: saved release/dist API response and original case-record equivalence",
              "normalization": ["frontmatter.markitai_processed only"],
              "baseline": old_identity, "candidate": new_identity,
              "checked_cases": len(records), "equivalent_cases": sum(item["equal"] for item in records),
              "native_responses_equal": sum(item["native"]["equal"] for item in records),
              "reference_responses_equal": sum(item["reference"]["equal"] for item in records),
              "all_equivalent": all(item["equal"] for item in records), "cases": records,
              "limits": ["Compares every serialized audit response field, preserving asset order and warnings; only the processing timestamp is omitted.",
                         "Rechecks source, expected-document and actual asset bytes against stored hashes; does not rerun conversion.",
                         "Does not inspect conversion fields omitted by both audit runners, such as elapsed time or output-path prefixes.",
                         "Build profile and source equivalence require the coordinator's artifact manifest; library content hashes are verified here.",
                         "Equivalent wrong or rejected outputs establish profile consistency, not parity with the reference or full semantic correctness."]}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> int:
    if not __debug__:
        raise RuntimeError("Do not disable audit assertions with Python -O")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=["formats", "html"], required=True)
    for name in ("baseline", "candidate", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    for name in ("baseline-sha", "candidate-sha"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    report = compare(args.kind, args.baseline.resolve(), args.candidate.resolve(),
                     args.baseline_sha, args.candidate_sha, args.output.resolve())
    print(json.dumps({key: report[key] for key in ("checked_cases", "equivalent_cases", "native_responses_equal", "reference_responses_equal", "all_equivalent")}, indent=2))
    return int(not report["all_equivalent"])


if __name__ == "__main__":
    raise SystemExit(main())
