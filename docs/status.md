# Project status

Marrow is experimental and unreleased. This page describes the repository at
the same Git revision and separates reachable behavior from direction.

| State | Meaning |
|---|---|
| Current | Implemented behavior documented by the current reference and tests. |
| Legacy | Implemented behavior that is intentionally excluded from the target architecture and should not be expanded. |
| Future | Unimplemented direction recorded under `docs/future/`; it is not a current contract. |

## Current

### Language

- Native parser and formatter for `.mw` source.
- Static scalar, resource, enum, sequence, local-tree, optional, and nominal
  store-identity types.
- Modules, functions, lexical bindings, control flow, structured errors,
  presence narrowing, and host-effect checks.
- Direct durable reads, assignments, deletion, ordered keyed iteration,
  managed indexes, and lexical transactions.
- Source declarations for supported changes to populated data.

### Execution and durable state

- A checked executable representation interpreted by a tree-walking runtime.
- Memory and redb implementations of the current ordered-tree substrate.
- One owning write-capable native-store process or session.
- Managed writes that maintain declared indexes and commit transactionally.
- Accepted declaration identities and a state-bound preview/apply workflow for
  supported data evolution.
- Typed inspection, integrity checking, backup, restore, and physical recovery
  commands.

### Developer tools

- `init`, `check`, `fmt`, `run`, `test`, `doctor`, `data`, `evolve`, `backup`,
  and `restore` command families.
- Text, JSON, and selected JSONL diagnostic/report forms.
- A downstream language-server repository using Marrow compiler APIs.
- Linux and macOS source builds with the pinned Rust toolchain.

### Known implementation limitations

- Argument labels are not rejected consistently on standard-library and some
  language-intrinsic or local-collection calls. A mislabeled call may check
  without producing an executable function body, or fail only at evaluation.
- `ErrorCode` validation is not preserved through every annotated boundary;
  parameters, returns, bare-local reassignment, local collections, keys,
  optional or uninitialized locals, and module constants can erase the
  refinement to `string`.

## Legacy

The following mechanisms are reachable but rejected as foundations for v1:

- `surface` declarations that repeat selected store fields and operations;
- generated collection/read/create/update/delete/action families;
- operation-tag HTTP routes and the current generated TypeScript client;
- `marrow serve`, `marrow client typescript`, and `init --client` as currently
  implemented;
- the user-facing storage-cost model and hidden-scan terminology; and
- application sessions centered on the surface model and current catalog
  lifecycle.

They are summarized in [Legacy mechanisms](legacy.md) so the current repository
remains understandable, but they are absent from the main learning path and
language manual. Bearer authentication in the experimental server is not
compiler-integrated path authorization.

## Future

The intended architecture includes:

- reproducible compiled and verified program images;
- stable compiler-owned semantic path identities;
- a neutral logical tree beneath one authorized path kernel;
- read-only store admission and explicit atomic activation;
- a source-defined portable standard library above minimal host intrinsics;
- ordinary exported functions and generated local UI bindings;
- explicit public path projections and typed path capabilities; and
- a served profile that preserves embedded program semantics.

The [future index](future/) describes this direction without defining current
syntax or blocking implementation on speculative contracts.

## Not current

Marrow does not currently provide bytecode or native code generation,
compiler-integrated path authorization, path-native URI publication, a
supported packaged desktop host, concurrent multi-writer service deployment,
replication, high availability, signed releases, or institutional protocol
conformance.

## Trust boundaries

- Filesystem permissions and the host process protect local store files.
- Checksums and structural verification detect selected corruption; they do
  not authenticate hostile storage.
- Encryption at rest is delegated to the filesystem or substrate.
- TLS, authentication, identity providers, operator credentials, and hardware
  durability are deployment responsibilities.
- Static checking cannot establish application intent, correct policy design,
  regulatory compliance, or freedom from external side channels.
