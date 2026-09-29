"""Typed Python API shared by synchronous and asynchronous callers."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Protocol

from . import _native

OutputProfileName = Literal["rag", "obsidian", "okf"]

__all__ = [
    "ConversionError", "ConversionOutput", "ConversionUsage", "FetchError",
    "NoModelConfiguredError", "OutputProfileName", "aconvert", "convert",
    "enable_worker_processes",
]


class ConfigModel(Protocol):
    def model_dump(self, *, mode: str) -> dict[str, Any]: ...


class ConversionError(RuntimeError):
    """A conversion failed; code retains the native error category."""

    def __init__(
        self, message: str, *, code: str = "conversion_error",
        usage: ConversionUsage | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.usage = usage


class FetchError(Exception):
    """An HTTP source could not be fetched."""

    code = "fetch_error"
    usage: ConversionUsage | None = None


class NoModelConfiguredError(ValueError):
    """LLM processing was requested without a configured model."""

    code = "no_model_configured"
    usage: ConversionUsage | None = None


@dataclass
class ConversionUsage:
    cost_usd: float = 0.0
    requests: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    by_model: dict[str, dict[str, Any]] = field(default_factory=dict)

    @classmethod
    def from_usage_dict(
        cls, cost_usd: float, by_model: dict[str, dict[str, Any]]
    ) -> ConversionUsage:
        return cls(
            cost_usd=cost_usd,
            requests=sum(int(row.get("requests", 0)) for row in by_model.values()),
            input_tokens=sum(int(row.get("input_tokens", 0)) for row in by_model.values()),
            output_tokens=sum(int(row.get("output_tokens", 0)) for row in by_model.values()),
            by_model=by_model,
        )


@dataclass
class ConversionOutput:
    source: str
    markdown: str
    llm_markdown: str | None = None
    frontmatter: dict[str, Any] = field(default_factory=dict)
    output_path: Path | None = None
    llm_output_path: Path | None = None
    assets: list[Path] = field(default_factory=list)
    screenshots: list[Path] = field(default_factory=list)
    images: list[dict[str, Any]] = field(default_factory=list)
    usage: ConversionUsage = field(default_factory=ConversionUsage)
    skip_reason: str | None = None
    duration: float = 0.0
    warnings: list[str] = field(default_factory=list)


def enable_worker_processes() -> None:
    """Compatibility hook; Rust handles its own execution without subprocesses."""


def _result(response: str) -> ConversionOutput:
    envelope = json.loads(response)
    if not envelope["ok"]:
        error = envelope["error"]
        code, message = error["code"], error["message"]
        raw_usage = error.get("usage")
        usage = ConversionUsage(**raw_usage) if raw_usage is not None else None
        if code == "fetch_error":
            exception = FetchError(message)
        elif code == "no_model_configured":
            exception = NoModelConfiguredError(message)
        elif code in {"invalid_input", "invalid_json", "config_error"}:
            exception = ValueError(message)
        elif code == "not_found":
            exception = FileNotFoundError(message)
        elif code == "is_directory":
            exception = IsADirectoryError(message)
        elif code == "io_error":
            exception = OSError(message)
        else:
            exception = ConversionError(message, code=code)
        # Preserve built-in and public exception categories. None means the
        # producer supplied no accounting, not that the failed call was free.
        exception.usage = usage
        raise exception
    data = envelope["result"]
    data["usage"] = ConversionUsage(**data.get("usage", {}))
    for key in ("output_path", "llm_output_path"):
        data[key] = Path(data[key]) if data.get(key) is not None else None
    for key in ("assets", "screenshots"):
        data[key] = [Path(value) for value in data.get(key, [])]
    return ConversionOutput(**data)


def _convert(
    source: str | Path,
    *,
    output_dir: str | Path | None,
    config: Mapping[str, Any] | ConfigModel | None,
    llm: bool | None,
    ocr: bool | None,
    screenshot: bool | None,
    alt: bool | None,
    desc: bool | None,
    profile: OutputProfileName | None,
) -> ConversionOutput:
    if not isinstance(source, (str, Path)):
        raise TypeError("source must be a string or pathlib.Path")
    if config is not None:
        config = dict(config) if isinstance(config, Mapping) else config.model_dump(mode="json")
    options = {
        "output_dir": str(output_dir) if output_dir is not None else None,
        "config": config,
        "llm": llm,
        "ocr": ocr,
        "screenshot": screenshot,
        "alt": alt,
        "desc": desc,
        "profile": profile,
    }
    request = json.dumps({"source": str(source), "options": options}, ensure_ascii=False)
    return _result(_native.convert_json(request))


def convert(
    source: str | Path,
    *,
    output_dir: str | Path | None = None,
    config: Mapping[str, Any] | ConfigModel | None = None,
    llm: bool | None = None,
    ocr: bool | None = None,
    screenshot: bool | None = None,
    alt: bool | None = None,
    desc: bool | None = None,
    profile: OutputProfileName | None = None,
) -> ConversionOutput:
    """Convert a file or URL, releasing the GIL while the Rust engine runs."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        pass
    else:
        raise RuntimeError(
            "markitai.convert() cannot be called from a running event loop; "
            "use `await markitai.aconvert(...)` instead"
        )
    return _convert(
        source, output_dir=output_dir, config=config, llm=llm, ocr=ocr,
        screenshot=screenshot, alt=alt, desc=desc, profile=profile,
    )


async def aconvert(
    source: str | Path,
    *,
    output_dir: str | Path | None = None,
    config: Mapping[str, Any] | ConfigModel | None = None,
    llm: bool | None = None,
    ocr: bool | None = None,
    screenshot: bool | None = None,
    alt: bool | None = None,
    desc: bool | None = None,
    profile: OutputProfileName | None = None,
) -> ConversionOutput:
    """Convert without blocking the event loop; work runs in a native thread.

    Cancelling the await abandons the result but does not interrupt a running
    conversion or roll back files it writes.
    """
    return await asyncio.to_thread(
        _convert, source, output_dir=output_dir, config=config, llm=llm, ocr=ocr,
        screenshot=screenshot, alt=alt, desc=desc, profile=profile,
    )
