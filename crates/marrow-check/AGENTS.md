# marrow-check Contributor Notes

This crate is the current semantic owner for resolution, types, presence and
effects, durable identity, lowering, evolution checks, and analysis facts. It
produces `CheckedProgram`, the executable artifact used by the current runtime;
that type is not automatically the final compiled-image architecture.

Semantic path facts originate here. As the path model is refounded, one typed
graph must own stable schema path identities. It derives source spellings,
concrete durable addresses, URI projections, authority regions, graph-version
evolution facts, and tooling views. Entry identities and store UIDs remain
distinct. Downstream crates may consume these facts but never recreate them
from names or formatted paths.

Do not introduce query plans, cost plans, CRUD families, or operation matrices
as central compiler semantics. Current surface facts are legacy implementation
and should be contained or removed, not expanded.

Diagnostics consist of a typed `Code` and payload; prose belongs only to the
render owner. Call targets, IDs, path effects, and spans are typed and compared
by value. Say “checked” or “witnessed,” not “proved,” unless a formal result is
published.

After changing inference, checking, analysis, hover, or another published
semantic fact, run `marrow check --compiler-dev <projectdir>` on each affected
clean fixture project. A `compiler.dev.unknown_type` warning blocks integration
until the missing checker-owned type or fact is fixed, or an existing typed
semantic owner establishes that the position is non-value and audit eligibility
is corrected. Do not baseline warnings, add source/path/span allowlists, or
silence them with lexical probe exceptions. The warning marks unresolved checker
recovery, not the explicit source `unknown` type. Do not recommend this hidden
maintainer mode to ordinary Marrow users.

Map: [docs/implementation/compiler.md](../../docs/implementation/compiler.md).
