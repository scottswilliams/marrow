# Packages

A package is a unit of Marrow source that another project imports through an
exact dependency edge.

## Today

A project is one `marrow.toml` manifest, whose only key is `edition`, and the
modules under `src/` ([projects](../tools/projects.md)). `marrow init` creates
the manifest and a first module and creates no store. There is no dependency
edge, no package cache, and no package identity.

## Direction

Marrow has a small reproducible package workflow based on local paths and
exact Git edges. There is no registry and no semantic-version solver.

### Dependency edges

Dependencies are exact and close the graph without a separate resolver.

- A local path edge names another package by path for development-time use.
- A public-HTTPS Git edge names a package by locator together with an exact full
  commit identifier and the expected package lineage. There is no version range,
  tag-following, or branch-tracking edge.
- The exact transitive manifests reachable through these edges already determine
  the complete dependency graph. Each edge resolves to one lineage at one
  material.

Exact edges close the graph, so there is no lock file. Two edges that resolve
to the same lineage at the same material are one instance. One lineage at two
different materials is a conflict and is rejected. A release prefers Git edges
to path edges so that its provenance is reviewable and reacquirable.

### Package identity

A package has a lineage and a material. Lineage is its stable identity across
acquisitions. Material is a content identity over what the package means to a
consuming program: its manifest, edition, import aliases, dependency edges, and
source. Where the bytes came from (locator, revision, checkout, time) is
recorded separately and does not affect identity. A content hash establishes
exact-byte integrity; it says nothing about the author or the review.

### Constraints

Explicit add, update, and fetch operations may use the network. Check, build,
test, format, run, and image loading read only a verified local cache. Cache
material is rehashed before use, so a verified cache reproduces the same
material offline.

A dependency alias is an explicit identifier that imports use unchanged.
Importing a package grants no host access, creates no durable place, and runs
no initializer. A dependency is pure source: it declares no store and runs no
code at build time. Its private declarations stay image-local; stable ledger
entries appear only at a public, durable, or generated-host boundary.

Fetching uses the installed system Git and is not a sandbox; integrity comes
from rehashing what was fetched. Marrow embeds no Git implementation and
defines no transport.

A durable package, with abstract root requirements that an application mounts,
is later work. The package system serves pure libraries first.

## Evidence

One graph builds to identical image bytes from the verified offline cache,
fails closed on corrupted material and on one lineage at two materials, and
runs offline after explicit acquisition. A project that imports one real Git
package passes init, check, format, test, run, and update, then rebuilds from
the cache alone.
