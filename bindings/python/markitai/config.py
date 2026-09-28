"""Mutable configuration snapshots backed by native defaults and validation.

This module implements the public configuration entry point without bringing a
Python validation framework into the native package.
"""

from __future__ import annotations

import copy
import json
from enum import Enum
from pathlib import Path
from typing import Any

from . import _native


def _json_value(value: Any) -> Any:
    if isinstance(value, _Section):
        return value._data
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, Enum):
        return value.value
    raise TypeError(f"Configuration value of type {type(value).__name__} is not JSON compatible")


class _Section:
    def __init__(self, data: dict[str, Any]) -> None:
        object.__setattr__(self, "_data", data)

    def __getattr__(self, key: str) -> Any:
        try:
            value = object.__getattribute__(self, "_data")[key]
        except KeyError:
            raise AttributeError(key) from None
        return _Section(value) if isinstance(value, dict) else value

    def __setattr__(self, key: str, value: Any) -> None:
        self._data[key] = value._data if isinstance(value, _Section) else value

    def model_dump(self, *, mode: str = "python", **kwargs: Any) -> dict[str, Any]:
        if kwargs:
            raise TypeError("model_dump supports only the mode argument in the native package")
        if mode == "json":
            return json.loads(json.dumps(self._data, default=_json_value))
        if mode != "python":
            raise ValueError("mode must be 'python' or 'json'")
        return copy.deepcopy(self._data)


class MarkitaiConfig(_Section):
    """Configuration with mutable sections, model_dump, and model_copy.

    Keyword sections are merged into Rust's defaults. Values are validated on
    construction and again when converting. This is not a Pydantic BaseModel;
    schema generation, validators, and Pydantic's complete method set are not
    part of this compatibility layer.
    """

    def __init__(self, **sections: Any) -> None:
        snapshot = _native.config_json(json.dumps(sections, default=_json_value))
        super().__init__(json.loads(snapshot))

    def model_copy(
        self, *, update: dict[str, Any] | None = None, deep: bool = False
    ) -> MarkitaiConfig:
        result = object.__new__(type(self))
        data = copy.deepcopy(self._data) if deep else self._data.copy()
        if update:
            data.update(update)
        object.__setattr__(result, "_data", data)
        return result

    @classmethod
    def model_validate(cls, value: dict[str, Any]) -> MarkitaiConfig:
        return cls(**value)

    @classmethod
    def model_validate_json(cls, value: str | bytes) -> MarkitaiConfig:
        return cls.model_validate(json.loads(value))

    def model_dump_json(self) -> str:
        return json.dumps(self.model_dump(mode="json"), ensure_ascii=False)
