# ADR 0001: one native conversion core

Status: accepted, 2026-09-28.

The product needs a standalone CLI, three language bindings, and substantial
startup and distribution savings. A Rust wrapper around Python would retain
the old runtime and dependency costs, so conversion belongs in Rust.

The core exposes typed Rust values and a JSON transport boundary for native
adapters. Go uses an owned C buffer with an explicit free function; Node and
Python use their native extension APIs. CLI JSON stays a separate compatibility
envelope because its batch outcomes differ from the library's single result.

Fresh Rust design does not mean freely changing user behavior. Compatibility
is measured against the reference source and tests. Unsupported paths must
fail clearly until implemented; they cannot quietly degrade into plain text.

The [Bun rewrite retrospective](https://bun.com/blog/bun-in-rust) informed the
independent review and checkpoint workflow. Our architecture is intentionally
designed around Rust rather than copying the reference module structure.
