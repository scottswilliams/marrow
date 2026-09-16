# Packages

Remote acquisition would let a program depend on source it does not have a copy
of, without a registry, a version solver, or a package identity in the image.

## Today

A project names other local directories of Marrow source by relative path under
a consumer-chosen alias ([projects](../tools/projects.md#dependencies)). Both
trees are captured together under one set of bounds and compiled by one
compiler; a dependency is pure source, read and never written, and supplies no
initializer, build script, or host access. Nothing is fetched, cached, or
resolved over a network.

## Direction

Acquire a dependency's source from a remote location by exact revision, and
verify what was acquired against that revision before it is captured. The
acquired tree then enters the same capture and the same bounds as a local one,
so acquisition is the only new step.

Acquisition is an explicit operation, never a side effect of checking,
compiling, formatting, testing, or running: a build with its sources already
present is offline and reproducible. A verified cache is content-addressed and
is read like any other local tree.

A dependency graph deeper than one edge waits for evidence from a real caller.
Admitted later, it stays bounded and acyclic under the same single capture
budget.

## Deferred

A registry, a version-range solver, a lock file, durable package mounting, and
package lineage in the program image are not beta requirements. The image's
reserved package lineage stays reserved: a local dependency's modules are
alias-rooted, which already tells two exports apart, and a package identity
exists only where acquisition establishes one.

## Evidence

A project acquires a library by exact revision, rebuilds offline from the
verified cache with no network operation, and produces the same image bytes as a
build from a local copy of that revision. A revision that does not verify, is
absent, or is mutated under the cache fails with bounded work and a diagnostic
naming the dependency.
