# Packages

This page is future direction. The current `marrow.json` project file and
store-projected `marrow.lock` are not the package model.

## Goal

Marrow should have a small reproducible package workflow based on Git and local
paths rather than a registry or semantic-version solver.

The intended artifacts are a source manifest, one complete exact dependency
lock owned by the root package, and a separate source-controlled ledger for
stable declaration identities. A Git URL locates bytes; package lineage, exact
content snapshot, dependency graph, and declaration identity remain distinct.

## Constraints

- Explicit add, update, fetch, and vendor operations may use the network.
  Normal check, build, test, format, run, and image loading do not.
- One package lineage resolves to one exact snapshot in a program image.
- Cache and vendor material are untrusted and rehashed before use.
- Importing a package grants no host access, creates no durable root, and runs no
  initializer.
- Dependencies cannot run build scripts, Git hooks, compiler plugins, native
  setup, or proc-macro equivalents.
- Content hashes establish exact-byte integrity and reproducibility, not author
  identity, review quality, or supply-chain trust.

Development path overrides must be visibly marked and refused by a release-
locked artifact policy. A release replaces them with reviewed exact Git
snapshots; vendoring materializes an existing lock and does not silently turn a
live path override into release provenance.

Durable packages with abstract root requirements and application-owned mounts
are later work. The beta package system can be useful and complete for pure
libraries without solving reusable durable ownership.

## Evidence target

The same locked program must build to identical image bytes from verified cache
and vendor material, fail closed on corrupted or conflicting graphs, and run
offline after explicit acquisition.
