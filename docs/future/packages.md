# Packages

This page is future direction. The current `marrow.json` project file and
store-projected `marrow.lock` are not the package model.

## Goal

Marrow should have a small reproducible package workflow based on Git and local
paths rather than a registry or semantic-version solver.

The intended artifacts are a source manifest, one complete exact dependency
lock owned by the root package when dependencies exist, and a separate
source-controlled ledger when declarations first need stable identity. A Git
URL locates bytes; package lineage, exact content snapshot, dependency graph,
and declaration identity remain distinct.
Initializing a storeless project should create only source and reproducibility
artifacts. It should not create a store, catalog, backend selection, generated
client, connection setting, or durable deployment.

## Constraints

- Explicit add, update, fetch, and vendor operations may use the network.
  Normal check, build, test, format, run, and image loading do not.
- Every source manifest explicitly declares its supported language edition;
  parsing does not inherit a moving toolchain default.
- A Git dependency records the expected source-controlled package lineage as
  well as its locator and immutable revision.
- After an accepted root manifest or declaration-identity edit, a separate
  network-free lock action rebinds the unchanged graph to the new root digests;
  normal build commands never repair a stale lock implicitly.
- One package lineage resolves to one exact snapshot in a program image.
- Dependency aliases are explicit source identifiers used unchanged by imports;
  package names are metadata and are not normalized into aliases.
- Cache and vendor material are untrusted and rehashed before use.
- Importing a package grants no host access, creates no durable root, and runs no
  initializer.
- Stable ledger entries are required only at public, durable, or generated-host
  identity boundaries; private implementation declarations remain image-local.
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
offline after explicit acquisition. Graph Report must use one real exact Git
package through initialization, check, format, source tests, build, run, update,
vendor, and offline rebuild without acquiring a store or ambient capability.
