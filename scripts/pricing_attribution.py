"""Offline exact-source attribution for the compiled bounded token-price catalog."""
from pathlib import Path
import hashlib
import json
import stat

NAMES = ("LiteLLM-LICENSE", "README.md", "provenance.json", "source-rows.json")
MAX_FILE = 1024 * 1024


def pricing_files(root):
    """Return verified relative package paths and bytes; never download or infer notices."""
    directory = Path(root) / "licenses/pricing"
    # Neither directory nor files may redirect package provenance outside source.
    for parent in [directory.parent, directory]:
        if not stat.S_ISDIR(parent.lstat().st_mode):
            raise RuntimeError("Pricing attribution directory is not a regular directory")
    values = {}
    for name in NAMES:
        path = directory / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= MAX_FILE:
            raise RuntimeError("Pricing attribution must contain bounded regular source files")
        value = path.read_bytes()
        if len(value) != info.st_size:
            raise RuntimeError("Pricing attribution changed during reading")
        values[name] = value
    provenance = json.loads(values["provenance.json"])
    if provenance.get("schema_version") != 1 or provenance.get("license_file") != "LiteLLM-LICENSE":
        raise RuntimeError("Unsupported pricing attribution manifest")
    for name, key in [("LiteLLM-LICENSE", "license_sha256"), ("source-rows.json", "source_rows_sha256")]:
        if hashlib.sha256(values[name]).hexdigest() != provenance.get(key):
            raise RuntimeError("Pricing attribution source digest differs from provenance")
    rows = json.loads(values["source-rows.json"])
    expected = provenance.get("rows")
    if not isinstance(rows, dict) or not isinstance(expected, list) or set(rows) != {item["model"] for item in expected}:
        raise RuntimeError("Pricing attribution model inventory differs from provenance")
    return {"licenses/pricing/" + name: value for name, value in values.items()}


def stage_pricing_files(root, destination):
    files = pricing_files(root)
    record = {}
    for relative, content in files.items():
        target = Path(destination) / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as stream:
            stream.write(content)
        if target.read_bytes() != content:
            raise RuntimeError("Packaged pricing attribution differs from source")
        record[relative] = {"bytes": len(content), "sha256": hashlib.sha256(content).hexdigest()}
    return record
