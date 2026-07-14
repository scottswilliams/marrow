# Project status

Marrow is experimental and unreleased. This page describes the repository at
the same Git revision and separates reachable behavior from direction.

| State | Meaning |
|---|---|
| Current | Implemented behavior documented by the current reference and tests. |
| Legacy | Reachable behavior that is being removed and must not shape replacement architecture. |
| Future | Unimplemented direction under `docs/future/`; it is not current syntax or a guarantee. |

## Current

### Language and checking

- Native parser and formatter for `.mw` source.
- Static scalar, resource, enum, sequence, local-tree, optional, and nominal
  store-identity types.
- Modules, functions, lexical bindings, control flow, structured errors,
  presence narrowing, and host-effect checks.
- Direct durable reads, assignments, deletion, ordered keyed iteration,
  managed indexes, and lexical transactions.
- Source declarations for selected changes to populated data.

### Execution and durable state

- A checked in-memory executable representation interpreted by a tree-walking
  runtime; no bytecode or native compiler backend.
- Memory and redb implementations of the current ordered-tree API.
- One owning write-capable native-store process or session.
- Managed writes that maintain declared indexes and commit transactionally.
- Accepted declaration identities and state-bound preview/apply for supported
  evolution.
- Typed inspection, integrity, backup, restore, and physical recovery commands.

### Developer tools

- `init`, `check`, `fmt`, `run`, `test`, `doctor`, `data`, `evolve`, `backup`,
  and `restore` command families.
- Text, JSON, and selected JSONL diagnostic/report forms. (Experimental
  `serve`, TypeScript client generation, and client scaffolding remain
  reachable but are rejected product families; see Legacy below.)
- A downstream language-server repository using current Marrow semantic APIs.
- Linux and macOS source builds with the pinned Rust toolchain.

### Known implementation defects

- Argument labels are not rejected consistently on standard-library and some
  intrinsic/local-collection calls. A mislabeled call may check without an
  executable body or fail only during interpretation.
- `ErrorCode` validation is not preserved through every annotated boundary;
  several parameter, return, local, collection, key, optional, and constant
  paths erase the refinement to `string`.

These defects are replacement and containment inputs, not beta promises.

## Legacy and transitional architecture

The following behavior remains reachable until its owning deletion lane. It
must not be expanded or preserved for compatibility.

### Rejected product families

- `surface` declarations and their repeated field/operation model;
- generated collection/read/create/update/delete/action families;
- operation-tag HTTP routes, Bearer-authenticated experimental serving, and the
  generated TypeScript client;
- `marrow serve`, `marrow client typescript`, and `init --client` as currently
  implemented; and
- the user-facing storage-cost model and hidden-scan terminology.

### Transitional foundations

- the tree-walking interpreter and optional executable-body model;
- the resource/schema split and Rust-table standard library;
- store-projected catalog identity, current `marrow.lock`, automatic baseline,
  and current evolution lifecycle;
- managed indexes and privileged `nextId` allocation;
- durable writes outside an explicit outer transaction, nested transactions
  joining an outer transaction, and current host-effect handling in transactions;
- `ProjectSession` orchestration and the mixed compiler/runtime/store JSON model;
- redb plus the current recovery wrapper and engine-private file knowledge; and
- `marrow.json` is the current, transitional project model.

These are current facts and remain documented while reachable. The refounding
will remove their reference pages with their production code rather than keep a
permanent legacy manual.

## Future: v0.1 beta

The planned beta direction includes:

- an ordinary storeless language floor with algebraic data types, exhaustive
  patterns, real rank-1 parametric functions and types, generic local
  collections, modules, source tests, formatting, and editor facts; closures are
  deferred until a maintained program needs them;
- `marrow.toml` with exact path and Git dependency edges, a separate
  stable-identity ledger, and a verified offline cache; no dependency lock or
  vendoring unless a moving-resolution case earns one;
- one reproducible ProgramImage, an independent bounded verifier, and a bytecode
  VM qualified on one target;
- closed value and durable representations — dense products and sparse resources
  — with typed ordered durable trees, separate root and branch topology, narrow
  compiler-maintained nonunique and unique indexes, explicit transactions,
  application-owned counters and secondary trees, and bounded nested traversal;
- compiler-described effects, exact accepted execution binding, and one
  authority-checking path kernel;
- one qualified private native engine with no public backend choice;
- StoreId, durable-contract and executable-binding generations, read-only
  admission, crash-recoverable activation, metadata-only additive activation with
  one bounded add-index transition, logical backup, and fresh-store restore; and
- a personal local application whose durable model is proven terminal-first and
  whose release gate is a generated strict TypeScript client supervised by an
  Electron/Node process.

[Future direction](future/) records goals and boundaries without defining
unimplemented syntax or exact formats.

## Not current

Marrow does not currently provide general-purpose language completeness,
third-party packages, compiled images, an independent verifier or VM,
compiler-integrated runtime path authority, executable/store binding, a
supported packaged desktop application, public path publication, a supported
served profile, concurrent multi-writer deployment, replication, high
availability, signed releases, or institutional protocol/compliance evidence.

## Current trust boundaries

- Filesystem permissions and the host process protect local store files.
- Checksums and structural checks detect selected corruption; they do not
  authenticate hostile storage or prove full application validity.
- Encryption at rest is delegated to the filesystem or substrate.
- TLS, authentication, identity providers, operator credentials, and hardware
  durability are deployment responsibilities.
- Static checking cannot establish application intent, correct policy design,
  regulatory compliance, or absence of external side channels.
