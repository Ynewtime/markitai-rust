"""Offline complete terms and exact compiled-data provenance for the Codex catalog."""
from pathlib import Path
import hashlib
import json
import stat

NAMES = ("LICENSE", "NOTICE", "README.md", "provenance.json", "models.json")
COMPILED = "crates/markitai-core/src/subscription/chatgpt/models.json"
MAX_FILE = 1024 * 1024


def bounded_regular(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= MAX_FILE:
        raise RuntimeError("Codex attribution requires bounded regular source files")
    value = path.read_bytes()
    if len(value) != info.st_size:
        raise RuntimeError("Codex attribution changed during reading")
    return value


def codex_files(root):
    root = Path(root)
    directory = root / "licenses/codex"
    for path in [directory.parent, directory]:
        if not stat.S_ISDIR(path.lstat().st_mode):
            raise RuntimeError("Codex attribution directory is not a regular directory")
    values = {name: bounded_regular(directory / name) for name in NAMES}
    provenance = json.loads(values["provenance.json"])
    if (provenance.get("schema_version") != 1 or provenance.get("license_file") != "LICENSE"
            or provenance.get("notice_file") != "NOTICE"
            or provenance.get("compiled_catalog") != COMPILED):
        raise RuntimeError("Unsupported Codex attribution manifest")
    for name, key in [("LICENSE", "license_sha256"), ("NOTICE", "notice_sha256"), ("models.json", "catalog_sha256")]:
        if hashlib.sha256(values[name]).hexdigest() != provenance.get(key):
            raise RuntimeError("Codex attribution digest differs from provenance")
    path = root
    for component in Path(COMPILED).parts[:-1]:
        path /= component
        if not stat.S_ISDIR(path.lstat().st_mode):
            raise RuntimeError("Compiled Codex catalog must remain inside regular source directories")
    if bounded_regular(root / COMPILED) != values["models.json"]:
        raise RuntimeError("Packaged Codex catalog differs from the compiled data")
    models = json.loads(values["models.json"])["models"]
    if [model["slug"] for model in models] != provenance.get("models"):
        raise RuntimeError("Codex model inventory differs from provenance")
    return {"licenses/codex/" + name: content for name, content in values.items()}


def stage_codex_files(root, destination):
    record = {}
    for relative, content in codex_files(root).items():
        path = Path(destination) / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(content)
        if path.read_bytes() != content:
            raise RuntimeError("Packaged Codex attribution differs from source")
        record[relative] = {"bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()}
    return record
