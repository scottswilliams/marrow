# Project status

Marrow is experimental and unreleased. This page describes the repository at
the same Git revision and separates current behavior from direction.

The beta line began at lane B00 with a deliberate capability trough: the
entangled prototype semantic owners were deleted, and the trustworthy decoupled
parts were retained to be built on. The verticals listed under Future are being
refounded lane by lane; a feature is absent until its lane lands it.

| State | Meaning |
|---|---|
| Current | Implemented behavior documented by the current reference and tests. |
| Future | Unimplemented direction under `docs/future/`; it is not current syntax or a guarantee. |

## Current

The retained beta workspace is five crates: the diagnostic-code registry
(`marrow-codes`), the syntax owner (`marrow-syntax`), the ordered-byte storage
engine (`marrow-store`), the pure project-input owner (`marrow-project`), and a
thin `marrow` CLI.

### Language and tooling

- Native lexer, parser, and formatter for `.mw` source, with typed parse
  diagnostics.
- One pure project-input owner: the closed `marrow.toml` manifest schema
  (required explicit `edition`), deterministic contained discovery over `src`,
  path-derived module identity, and an immutable project input. See
  [Projects](tools/projects.md).
- `marrow init` creates a new project; `marrow fmt` formats a single `.mw` file
  or every module of a project directory (`--check`/`--write`); `marrow
  --version` and `marrow --help`. Every other command name is recognized but
  reports `cli.command_unsupported` until its refounding lane lands.
- Linux and macOS source builds with the pinned Rust toolchain.

### Storage engine

- A private ordered-byte engine contract with in-memory and redb backends under
  one conformance suite, plus the logical key/value/civil-date codecs. The
  engine has no source-language consumer on the beta line yet; the compiler,
  path kernel, and runtime that drive it are refounded starting at the thesis
  tracer lane.

### Deleted at B00

The prototype's entangled owners were deleted on the beta line and must not
shape the replacement architecture. Each returns only through its refounding
lane, rebuilt as a new owner:

- the `surface` stack (declarations, generated CRUD/collection/action families,
  operation-tag HTTP routes, experimental serving, and the generated TypeScript
  client), and the user-facing storage-cost model;
- the tree-walking interpreter, the composed project-session owner, the
  resource/schema split, the
  store-projected catalog and current evolution lifecycle, managed indexes and
  `nextId`, and the mixed compiler/runtime/store JSON model;
- the `check`/`run`/`test`/`data`/`doctor`/`evolve`/`serve`/`client`/`backup`/
  `restore` command families and the store's logical/admission/catalog/
  backup layers (the byte engine and its codecs are retained; `init` returns
  refounded at B01 as a pure-project-owner scaffold with no store); and
- the redb page-level recovery probe and the process-global panic-hook swap.

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
