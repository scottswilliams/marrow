# Source standard library

This page records future direction. The current toolchain supplies no `std::`
modules. A project-declared `std::` path follows ordinary project-module
resolution and remains project code.

## Goal

Portable library behavior should be ordinary Marrow source compiled and
verified with applications. Rust remains the trusted bootstrap for primitive
operations, compiler, image verifier, VM, host imports, path kernel, lifecycle,
and physical engine.

The first source-defined layer should include `Option` and `Result` helpers,
generic collection combinators, bounded text utilities, comparison helpers, and
other pure behavior whose implementation does not need privileged runtime state.

## Constraints

- Core package lineage and identity are toolchain-pinned and cannot be
  impersonated by source spelling.
- Source helpers use the same rank-1 generics, effects, image, verifier, and VM
  as application code, and only the implemented procedural floor; closures are not
  a prerequisite and remain deferred.
- Minimal VM intrinsics own only operations that cannot be expressed portably or
  cannot meet measured bounds in source.
- Compiler facts remain compiler-owned; source tools may render them but cannot
  rederive types, paths, effects, or update verdicts.
- The compiler can diagnose a project when optional bundled tools are missing.
- Project code is never an ambient compiler plugin.

## Dogfood boundary

The source standard library, Graph Report, and Club Locker business logic should
be written in Marrow. The compiler, parser, package acquirer, canonical encoder,
verifier, VM, path kernel, engine adapter, recovery worker, and lifecycle remain
Rust in v0.1. Self-hosting is not a beta goal.
