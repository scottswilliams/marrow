# marrow-project Contributor Notes

This crate owns current project discovery and configuration: `marrow.json`,
source roots, path-to-module resolution, and project digests. CLI and tooling
must share this loader.

Configuration errors use closed typed kinds and typed path payloads. Reject
absolute paths, traversal, unsupported glob syntax, and NUL at the boundary;
raw structs deny unknown fields. Source contains no `unwrap` or `panic`, and
exported types derive `Debug`.

The current configuration format is implementation state, not permanent
language law. Embedded and served profile semantics must originate in the
compiler/runtime design rather than ad hoc configuration branches.

Map: [docs/implementation/tooling.md](../../docs/implementation/tooling.md).
