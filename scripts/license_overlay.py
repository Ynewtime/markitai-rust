"""Validate and archive reviewed local upstream notices without network access.

A full text and a license notice are deliberately different states. Reviewed
historical originals can supply full terms while retaining historical provenance.
Hash checks do not establish the legal sufficiency of a distribution.
"""
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import tempfile

MAX_FILE = 16 * 1024 * 1024
MAX_TOTAL = 32 * 1024 * 1024
MAX_FILES = 512


def _json(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise RuntimeError("Duplicate license evidence JSON key")
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=pairs)


def _relative(value):
    if not isinstance(value, str) or not value or "\\" in value:
        raise RuntimeError("Invalid license evidence path")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in value.split("/")):
        raise RuntimeError("License evidence path escapes its directory")
    return path


def _read(root, name, expected=None):
    relative = _relative(name)
    path = root
    if root.is_symlink() or not root.is_dir():
        raise RuntimeError("License evidence root must be a real directory")
    for part in relative.parts:
        path /= part
        if path.is_symlink():
            raise RuntimeError("License evidence must not contain symlinks")
    if not stat.S_ISREG(path.lstat().st_mode) or path.stat().st_size > MAX_FILE:
        raise RuntimeError("License evidence must be a bounded regular file")
    with path.open("rb") as stream:
        raw = stream.read(MAX_FILE + 1)
    if len(raw) > MAX_FILE:
        raise RuntimeError("License evidence file exceeds its bound")
    if expected is not None:
        size, digest = expected.get("bytes"), expected.get("sha256")
        if type(size) is not int or size < 0 or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise RuntimeError("Invalid license evidence identity")
        if len(raw) != size or hashlib.sha256(raw).hexdigest() != digest:
            raise RuntimeError("License evidence byte identity differs")
    return raw


def _repo(value):
    if not isinstance(value, str) or not value.startswith("https://github.com/"):
        raise RuntimeError("Unsupported upstream evidence repository")
    repo = value.removeprefix("https://github.com/").rstrip("/").removesuffix(".git")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise RuntimeError("Invalid upstream evidence repository")
    return repo



# Only these reviewed package identities may add externally hosted license terms.
_AUTHORITY_TERMS = {
    "Apache-2.0": {
        "url": "https://www.apache.org/licenses/LICENSE-2.0.txt",
        "sha256": "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
        "link": b"[Apache-2.0]: https://www.apache.org/licenses/LICENSE-2.0",
        "repository": "madsmtm/objc2",
        "packages": {
            ("dispatch2", "0.3.1", "8852b424193ca41602281b3d7540d7c8ed51e49a"),
            ("objc2-core-foundation", "0.3.2", "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b"),
            ("objc2-core-graphics", "0.3.2", "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b"),
            ("objc2-image-io", "0.3.2", "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b"),
            ("objc2-vision", "0.3.2", "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b"),
        },
    },
    "MPL-2.0": {
        "url": "https://www.mozilla.org/media/MPL/2.0/index.f75d2927d3c1.txt",
        "sha256": "3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04",
        "link": b"https://mozilla.org/MPL/2.0/",
        "repository": "servo/stylo",
        "packages": {("selectors", "0.38.0", "572ecba2d1600e7c3d490586692a209faf703baa")},
    },
}
_MIT_ORIGINAL_URL = "https://raw.githubusercontent.com/madsmtm/objc2/9961247c1a82027d6edbe6c516011b1363b9354c/LICENSE.txt"
_MIT_ORIGINAL_SHA = "e353f37b12aefbb9f9b29490e837cfee05d9bda70804b3562839a3285c1df1e5"
_MIT_TARGETS = {
    ("objc2", "0.6.4"): "8852b424193ca41602281b3d7540d7c8ed51e49a",
    ("objc2-encode", "4.1.0"): "8d214f5477365ffcbcbb7de058c86ed9a518efb7",
    ("objc2-foundation", "0.3.2"): "7b1abfd750a2cacaea71d6a56ecfb83cb7de560b",
}
_MIT_PROOFS = {'https://github.com/madsmtm/objc2/commits/8d214f5477365ffcbcbb7de058c86ed9a518efb7/LICENSE.txt': '0a1efed6cc84aac1d931fe09facf2883ae2fb13caa4e131ddc3bf61aca0d9e3a', 'https://github.com/madsmtm/objc2/commit/cfb199226661ec6e4939fcd7cd7531b3c5453076.patch': 'b4ecc7d6db95a78e1cf777636049b4a9156783f326edf016a1461b24c84d1872', 'https://github.com/madsmtm/objc2/commit/9961247c1a82027d6edbe6c516011b1363b9354c.patch': '13127f39976ceecdadb86870d2448867fba8601ad21a376724f242fbd67e86fb', 'https://github.com/madsmtm/objc2/commits/8852b424193ca41602281b3d7540d7c8ed51e49a/LICENSE.txt': 'cce3f980b9088bf6533102997510603f50a5fbdda6fd34c055f797777fc20e0b', 'https://github.com/madsmtm/objc2/commits/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b/LICENSE.txt': 'ab00bc3db74112a44b17ac776fab3a16bc6122ffc8ccf2708a80e5d96206b4de'}
_SELECTORS_ARCHIVE_SHA = "8adfa1c298912827b8a28b223b3b874357397ae706e6190acd9bf28cee99114d"


def _same_commit_source(asset, data, root, repo, commit):
    source_path = str(_relative(asset["source_path"]))
    if not source_path.startswith(root) or source_path not in data:
        raise RuntimeError("License source is outside the attributed commit")
    expected_url = "https://raw.githubusercontent.com/" + repo + "/" + commit + "/" + source_path[len(root):]
    full = data[source_path]
    if asset["source_url"] != expected_url or hashlib.sha256(full).hexdigest() != asset["source_sha256"]:
        raise RuntimeError("License source URL or hash differs")
    bounds = asset.get("source_byte_range")
    if bounds is not None:
        start, end = bounds.get("start_inclusive"), bounds.get("end_exclusive")
        if type(start) is not int or type(end) is not int or not 0 <= start < end <= len(full):
            raise RuntimeError("Invalid license notice byte range")
        full = full[start:end]
    return full


def _linked_terms(asset, entry, data, recorded, root, repo, commit):
    option = asset.get("license_option")
    authority = _AUTHORITY_TERMS.get(option)
    identity = (entry["name"], entry["version"], commit)
    if (authority is None or repo != authority["repository"] or identity not in authority["packages"]
            or option not in entry["declared_license"].split(" OR ")
            or entry["complete_license_options_present"] != [option]
            or asset["content_kind"] != "full_license_text"):
        raise RuntimeError("Unreviewed linked license authority or package")
    notices = [row for row in entry["assets"] if row["path"] == asset.get("notice_asset")]
    if (len(notices) != 1 or notices[0].get("source_kind", "same_commit") != "same_commit"
            or notices[0]["content_kind"] != "notice_only"):
        raise RuntimeError("Linked license requires its exact-version notice")
    notice = recorded(notices[0])
    if notice != _same_commit_source(notices[0], data, root, repo, commit):
        raise RuntimeError("Linked license notice differs from upstream source bytes")
    if authority["link"] not in notice:
        raise RuntimeError("Exact-version notice does not link this license authority")
    source_path = str(_relative(asset["source_path"]))
    raw = data.get(source_path)
    if (asset["source_url"] != authority["url"] or asset["source_sha256"] != authority["sha256"]
            or raw is None or hashlib.sha256(raw).hexdigest() != authority["sha256"]):
        raise RuntimeError("Linked authority URL or reviewed text hash differs")
    return raw


def _historical_terms(asset, entry, data, recorded, repo, commit):
    if (repo != "madsmtm/objc2" or _MIT_TARGETS.get((entry["name"], entry["version"])) != commit
            or entry["declared_license"] != "MIT" or entry["content_kind"] != "full_license_text"
            or entry["complete_license_options_present"] != ["MIT"]
            or asset.get("source_kind") != "historical_notice"
            or asset.get("provenance_verified") is not True
            or asset.get("legal_review") != "not_performed"
            or asset.get("content_kind") != "full_license_text"):
        raise RuntimeError("Historical license terms require their reviewed scope and provenance")
    # The current pinned release still declares MIT. Keeping its whole original
    # notice also preserves the upstream Apple SDK discussion without resolving it.
    root = f"upstream/{repo}/{commit}/"
    notices = [row for row in entry["assets"]
               if row.get("source_kind", "same_commit") == "same_commit"
               and row.get("content_kind") == "notice_only"]
    if len(notices) != 1:
        raise RuntimeError("Historical terms require their exact-version MIT notice")
    notice = recorded(notices[0])
    if (notice != _same_commit_source(notices[0], data, root, repo, commit)
            or hashlib.sha256(notice).hexdigest() != "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54"):
        raise RuntimeError("Historical terms current-version MIT notice differs")
    declared = data.get(root + entry["path_in_vcs"] + "/Cargo.toml", b"")
    if re.search(rb'^license[ \t]*=[ \t]*"MIT"[ \t]*(?:#[^\r\n]*)?$', declared, re.MULTILINE) is None:
        raise RuntimeError("Historical terms require the current package MIT declaration")
    source_path = str(_relative(asset["source_path"]))
    raw = data.get(source_path)
    if (asset["source_url"] != _MIT_ORIGINAL_URL or asset["source_sha256"] != _MIT_ORIGINAL_SHA
            or raw is None or hashlib.sha256(raw).hexdigest() != _MIT_ORIGINAL_SHA):
        raise RuntimeError("Historical original URL or reviewed text hash differs")
    expected = {
        "https://github.com/madsmtm/objc2/commits/" + commit + "/LICENSE.txt",
        "https://github.com/madsmtm/objc2/commit/cfb199226661ec6e4939fcd7cd7531b3c5453076.patch",
        "https://github.com/madsmtm/objc2/commit/9961247c1a82027d6edbe6c516011b1363b9354c.patch",
    }
    proofs = asset.get("proofs", [])
    if type(proofs) is not list or len(proofs) != 3 or {p.get("source_url") for p in proofs} != expected:
        raise RuntimeError("Historical supplement requires its target-anchored provenance")
    for proof in proofs:
        body = recorded(proof)
        if hashlib.sha256(body).hexdigest() != _MIT_PROOFS[proof["source_url"]]:
            raise RuntimeError("Historical provenance differs from reviewed upstream bytes")
    return raw


def _source_archives(manifest, data, recorded):
    records = manifest.get("source_archives", [])
    if type(records) is not list or len(records) > 1:
        raise RuntimeError("Unreviewed source archive inventory")
    result = []
    for row in records:
        raw = recorded(row)
        if (row.get("package") != "selectors" or row.get("version") != "0.38.0"
                or row.get("commit") != "572ecba2d1600e7c3d490586692a209faf703baa"
                or row.get("source_url") != "https://static.crates.io/crates/selectors/selectors-0.38.0.crate"
                or hashlib.sha256(raw).hexdigest() != _SELECTORS_ARCHIVE_SHA):
            raise RuntimeError("Source archive is not the reviewed exact package")
        # Fixed compressed digest bounds all archive members. Never extract paths.
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as archive:
            vcs = _json(archive.extractfile("selectors-0.38.0/.cargo_vcs_info.json").read())
            original = archive.extractfile("selectors-0.38.0/Cargo.toml.orig").read()
        if (vcs.get("git", {}).get("sha1") != row["commit"] or vcs.get("path_in_vcs") != "selectors"
                or original != data["local-evidence/selectors-0.38.0/Cargo.toml.orig"]):
            raise RuntimeError("Source archive version differs from license evidence")
        result.append(row)
    return result


def stage_overlay(source, destination, packages):
    """Return matching package text records; copy the complete verified evidence.

    Missing optional overlays leave the original source-only collector intact.
    A present but corrupt overlay fails closed before any evidence is copied.
    """
    source, destination = Path(source), Path(destination)
    if not source.exists() and not source.is_symlink():
        return {"packages": {}, "record": {"status": "absent"}}
    raw_inventory = _read(source, "inventory.json")
    inventory = _json(raw_inventory)
    if inventory.get("schema") != 1 or type(inventory.get("files")) is not list or len(inventory["files"]) > MAX_FILES:
        raise RuntimeError("Invalid license evidence inventory")
    data, identities, total = {}, {}, 0
    for entry in inventory["files"]:
        name = str(_relative(entry["path"]))
        if name in data or name == "inventory.json":
            raise RuntimeError("Duplicate or recursive license evidence inventory")
        data[name] = _read(source, name, entry)
        identities[name] = {"bytes": entry["bytes"], "sha256": entry["sha256"]}
        total += len(data[name])
        if total > MAX_TOTAL:
            raise RuntimeError("License evidence total exceeds its bound")
    observed = set()
    for index, path in enumerate(source.rglob("*")):
        if index > MAX_FILES * 4 or path.is_symlink():
            raise RuntimeError("Unexpected or excessive license evidence entries")
        if path.is_file():
            observed.add(path.relative_to(source).as_posix())
        elif not path.is_dir():
            raise RuntimeError("License evidence contains a special file")
    if observed != set(data) | {"inventory.json"}:
        raise RuntimeError("License evidence inventory is not exact")

    def recorded(entry):
        name = str(_relative(entry["path"]))
        if name not in data or identities[name] != {key: entry[key] for key in ["bytes", "sha256"]}:
            raise RuntimeError("License manifest identity differs from its inventory")
        return data[name]

    manifest = _json(data["manifest.json"])
    if manifest.get("schema") != 1 or type(manifest.get("packages")) is not list or len(manifest["packages"]) > MAX_FILES:
        raise RuntimeError("Invalid license overlay manifest")
    recorded(manifest["input_manifest"])
    available = {package["id"]: package for package in packages}
    matched, all_ids, all_assets = {}, set(), set()
    for entry in manifest["packages"]:
        key = entry["id"]
        if key in all_ids:
            raise RuntimeError("Duplicate license overlay package")
        all_ids.add(key)
        repo, commit = _repo(entry["repository"]), entry["commit"]
        if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise RuntimeError("License overlay requires an exact upstream commit")
        path = entry["path_in_vcs"]
        if path:
            _relative(path)
        root = f"upstream/{repo}/{commit}/"
        original = f"local-evidence/{entry['name']}-{entry['version']}/"
        upstream_manifest = root + (path + "/" if path else "") + "Cargo.toml"
        for proof in entry["evidence"]:
            recorded(proof)
        if any(name not in data for name in [original + ".cargo_vcs_info.json",
                                              original + "Cargo.toml.orig", upstream_manifest]):
            raise RuntimeError("Missing exact-version package evidence")
        vcs = _json(data[original + ".cargo_vcs_info.json"])
        if vcs.get("git", {}).get("sha1") != commit or vcs.get("path_in_vcs") != path:
            raise RuntimeError("License overlay VCS attribution differs")
        if data[upstream_manifest] != data[original + "Cargo.toml.orig"]:
            raise RuntimeError("Upstream package manifest differs from published original")
        kind = entry["content_kind"]
        options = entry["complete_license_options_present"]
        if kind not in {"full_license_text", "notice_only"} or type(options) is not list or any(not isinstance(value, str) or not value for value in options):
            raise RuntimeError("Invalid license text classification")
        if (kind == "full_license_text") != bool(options):
            raise RuntimeError("License notice cannot claim a complete license option")
        texts = []
        has_full_text = False
        for asset in entry["assets"]:
            raw = recorded(asset)
            asset_kind = asset.get("content_kind")
            if asset["path"] in all_assets or asset_kind not in {"full_license_text", "notice_only"}:
                raise RuntimeError("Duplicate or inconsistent upstream license asset")
            all_assets.add(asset["path"])
            source_kind = asset.get("source_kind", "same_commit")
            if source_kind == "same_commit":
                full = _same_commit_source(asset, data, root, repo, commit)
            elif source_kind == "linked_authority":
                full = _linked_terms(asset, entry, data, recorded, root, repo, commit)
            else:
                raise RuntimeError("Unreviewed license source kind")
            if full != raw:
                raise RuntimeError("License asset differs from its upstream source bytes")
            has_full_text |= asset_kind == "full_license_text"
            texts.append({"source": str(source / asset["path"]),
                          "path": (destination / asset["path"]).relative_to(destination.parent.parent).as_posix(),
                          **identities[asset["path"]], "origin": "verified_upstream_overlay",
                          "source_url": asset["source_url"], "source_kind": source_kind,
                          "content_kind": asset_kind})
        if not texts:
            raise RuntimeError("License classification differs from complete source texts")
        supplements = entry.get("supplemental", [])
        if type(supplements) is not list or len(supplements) > 1:
            raise RuntimeError("Unreviewed historical supplement inventory")
        for asset in supplements:
            raw = recorded(asset)
            if asset["path"] in all_assets:
                raise RuntimeError("Duplicate historical license supplement")
            all_assets.add(asset["path"])
            if _historical_terms(asset, entry, data, recorded, repo, commit) != raw:
                raise RuntimeError("Historical supplement differs from original source bytes")
            texts.append({"source": str(source / asset["path"]),
                          "path": (destination / asset["path"]).relative_to(destination.parent.parent).as_posix(),
                          **identities[asset["path"]], "origin": "verified_historical_license_text",
                          "source_url": asset["source_url"], "source_kind": "historical_notice",
                          "content_kind": "full_license_text", "provenance_verified": True,
                          "legal_review": "not_performed"})
            has_full_text = True
        if has_full_text != (kind == "full_license_text"):
            raise RuntimeError("License classification differs from complete source texts")
        package = available.get(key)
        if package is None:
            continue
        if any(package.get(field) != entry[field] for field in ["name", "version"]) or package.get("license") != entry["declared_license"] or _repo(package.get("repository")) != repo:
            raise RuntimeError("License overlay package metadata differs")
        directory = Path(package["manifest_path"]).parent
        for name in [".cargo_vcs_info.json", "Cargo.toml.orig"]:
            if _read(directory, name) != data[original + name]:
                raise RuntimeError("Current package source differs from exact-commit evidence")
        matched[key] = {"texts": texts, "complete_text": kind == "full_license_text",
                        "content_kind": kind, "commit": commit,
                        "full_text_gap": entry["unresolved_full_text_reason"],
                        "historical_provenance": bool(supplements)}
    source_archives = _source_archives(manifest, data, recorded)
    # All provenance and classification checks precede copying. The caller's
    # clean-source checks also bind the inventory itself to the build revision.
    data["inventory.json"] = raw_inventory
    for name, raw in sorted(data.items()):
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as stream:
            stream.write(raw)
    return {"packages": matched, "record": {
        "status": "verified", "manifest": {"path": (destination / "manifest.json").relative_to(destination.parent.parent).as_posix(), **identities["manifest.json"]},
        "matched_packages": len(matched),
        "complete_text_packages": sum(value["complete_text"] for value in matched.values()),
        "notice_only_packages": sum(not value["complete_text"] for value in matched.values()),
        "historical_text_packages": sum(value["historical_provenance"] for value in matched.values()),
        "legal_review": "not_performed",
        "source_archives": [{**row, "path": (destination / row["path"]).relative_to(destination.parent.parent).as_posix()} for row in source_archives],
        "archived_evidence_files": len(data),
        "scope": "Reviewed exact-version declarations and full terms, including explicitly identified historical originals; byte provenance and delivery checks only, no legal review"}}


def upstream_files(root):
    """Return validated offline evidence for CLI, wheel and npm archive payloads.

    The source snapshot supplies version provenance here. Static-Go packaging
    additionally passes the resolved Cargo packages to stage_overlay.
    """
    source = Path(root) / "licenses/upstream"
    if not source.exists():
        raise RuntimeError("Required upstream attribution directory is unavailable")
    with tempfile.TemporaryDirectory(prefix="markitai-upstream-notices-") as temporary:
        destination = Path(temporary) / "package/licenses/upstream"
        result = stage_overlay(source, destination, [])
        if result["record"]["status"] != "verified":
            raise RuntimeError("Upstream attribution did not validate")
        return {"licenses/upstream/" + path.relative_to(destination).as_posix(): path.read_bytes()
                for path in sorted(destination.rglob("*")) if path.is_file()}
