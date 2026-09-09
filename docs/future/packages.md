# Packages

Source reuse lets a program import a library without copying it or granting it
host or store access.

## Today

A project is one `marrow.toml` manifest and the modules under `src/`
([projects](../tools/projects.md)). There are no dependency edges or package
cache.

## Beta direction

Support an explicit local-path dependency on ordinary Marrow source. Resolve
names within modules and dependencies so two libraries need not rename their
private helpers to coexist. Checking, compiling, formatting, testing and running
consume one captured, bounded source graph without opening the network.

Importing source runs no initializer, build script or compiler plugin and grants
no filesystem, network, clock or durable access. Dependencies are pure source;
the application declares its store. Preserve the existing project identity and
publication owners where a dependency reaches an identity-bearing boundary.

## Deferred

Remote acquisition may later use exact Git revisions and verified cached
content. It needs a maintained caller, reproducible offline behavior, explicit
network operations, integrity checks and a bounded dependency graph. A registry,
version-range solver, durable package mounting, package lineage machinery and
new manifest or lock formats are not beta requirements.

## Evidence

Graph Report uses one separately located library, changes it, diagnoses a
missing or conflicting dependency and rebuilds offline without a store.
Repeated capture of identical inputs produces identical images. Missing,
cyclic, escaping or overlarge inputs fail with bounded work and useful
diagnostics. The library uses the same compiler and verifier as the application.
