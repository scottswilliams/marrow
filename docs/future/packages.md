# Packages

This page is future direction. The current `marrow.json` project file and
store-projected `marrow.lock` are not the package model.

## Goal

Marrow should have a small reproducible package workflow based on local paths and
exact Git edges rather than a registry or semantic-version solver.

Package identity is a pair of a stable package lineage and its semantic material.
Lineage is a durable identity for a package across acquisitions. Material is the
exact content that lineage resolves to in a given program image. Acquisition
provenance — a locator, a revision, a working checkout, and timestamps — locates
bytes and is recorded separately; it is not part of material. Semantic material
should be a content identity over exactly what the package means to a consuming
program — its manifest, language edition, import aliases, dependency edges, and
source content — and exclude the package's own lineage, locator, and revision.
The exact hashed payload is settled with a known-answer test when implemented.

Initializing a storeless project should create only source and reproducibility
artifacts. It should not create a store, catalog, backend selection, generated
client, connection setting, or durable deployment.

## Dependency edges

Dependencies are exact and close the graph without a separate resolver.

- A local path edge names another package by path for development-time use.
- A public-HTTPS Git edge names a package by locator together with an exact full
  commit identifier and the expected package lineage. There is no version range,
  tag-following, or branch-tracking edge.
- The exact transitive manifests reachable through these edges already determine
  the complete dependency graph. Each edge resolves to one lineage at one
  material.

Because the graph is already closed by exact edges, there is no dependency lock
file and no vendoring in the beta. A complete resolution lock or vendored source
tree is deferred and earned only if a concrete moving-resolution case later
requires one; neither is a beta requirement.

## Constraints

- Explicit add, update, and fetch operations may use the network. Normal check,
  build, test, format, run, and image loading are network-denied and consume only
  a verified local cache.
- Acquired material is held in a rebuildable offline cache. Cache material is
  untrusted and rehashed before use, so a verified cache reproduces the same
  material offline.
- Every source manifest explicitly declares its supported language edition;
  parsing does not inherit a moving toolchain default.
- Dependency aliases are explicit source identifiers used unchanged by imports;
  package names are metadata and are not normalized into aliases.
- Importing a package grants no host access, creates no durable place, and runs
  no initializer.
- Dependency packages are pure. A dependency declares no durable roots, branches,
  or indexes, performs no host imports, and cannot run build scripts, Git hooks,
  compiler plugins, native setup, package plugins, or proc-macro equivalents.
- Stable ledger entries are required only at public, durable, or generated-host
  identity boundaries; private implementation declarations remain image-local.
- Content hashes establish exact-byte integrity and reproducibility, not author
  identity, review quality, or supply-chain trust.

## Resolution model

There is no registry, semantic-version solver, or workspace feature unification.
A diamond in the graph deduplicates identical instances: two edges that resolve
to the same lineage at the same material are one instance. One lineage appearing
at two different materials is a conflict and is rejected rather than reconciled;
the beta admits no solver and no multiple nominal worlds for a single lineage.

Local path edges are a development-time convenience. A release should prefer exact
Git edges so that its dependency provenance is reviewable and reacquirable.

## Acquisition boundary

Acquisition uses the installed system Git as a development-time tool. Marrow does
not embed a Git implementation or define a custom transport. Acquisition is not
claimed to be a sandbox or quota boundary against a hostile remote; it offers
honest availability limits only, and integrity comes from rehashing material
against its expected lineage and revision rather than from confining the fetch.

Durable packages with abstract root requirements and application-owned mounts are
later work. The beta package system can be useful and complete for pure libraries
without solving reusable durable ownership.

## Evidence target

The same graph must build to identical image bytes from a verified offline cache,
fail closed on corrupted material or on one lineage at conflicting materials, and
run offline after explicit acquisition. A graph exercise must use one real exact
Git package through initialization, check, format, source tests, build, run, and
update, and then rebuild offline from the verified cache alone, without acquiring
a store or ambient capability.

This page states direction, not implementation evidence. [Project
status](../status.md) identifies what is current, legacy, and future; the
[reference](../language/) defines only current behavior.
