"""Validate and archive reviewed local upstream notices without network access.

A full text and a license notice are deliberately different states. Hash checks
establish byte provenance, not the sufficiency of a license for redistribution.
"""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat

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
        for asset in entry["assets"]:
            raw = recorded(asset)
            if asset["path"] in all_assets or asset["content_kind"] != kind:
                raise RuntimeError("Duplicate or inconsistent upstream license asset")
            all_assets.add(asset["path"])
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
            if full != raw:
                raise RuntimeError("License asset differs from its upstream source bytes")
            texts.append({"source": str(source / asset["path"]),
                          "path": (destination / asset["path"]).relative_to(destination.parent.parent).as_posix(),
                          **identities[asset["path"]], "origin": "verified_upstream_overlay",
                          "source_url": expected_url, "content_kind": kind})
        if not texts:
            raise RuntimeError("License overlay package has no text")
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
                        "full_text_gap": entry["unresolved_full_text_reason"]}
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
        "archived_evidence_files": len(data),
        "scope": "Reviewed local exact-version texts and provenance; notices alone do not resolve full-license-text gaps; no legal review"}}
