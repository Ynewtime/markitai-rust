"""Document conversion through the in-process Rust engine."""

from ._native import __version__
from .config import MarkitaiConfig
from .api import (
    ConversionError,
    ConversionOutput,
    ConversionUsage,
    FetchError,
    NoModelConfiguredError,
    OutputProfileName,
    aconvert,
    convert,
    enable_worker_processes,
)

__all__ = [
    "ConversionError", "ConversionOutput", "ConversionUsage", "FetchError",
    "NoModelConfiguredError", "OutputProfileName", "__version__", "aconvert",
    "convert", "enable_worker_processes", "MarkitaiConfig",
]
