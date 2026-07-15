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

The beta workspace is ten crates: the retained diagnostic-code registry
(`marrow-codes`), syntax owner (`marrow-syntax`), ordered-byte storage engine
(`marrow-store`), and pure project-input owner (`marrow-project`); the
refounded compiler pipeline (`marrow-compile`, `marrow-image`, `marrow-verify`,
`marrow-vm`) and path kernel (`marrow-kernel`); and the `marrow` CLI. The
[implementation map](implementation/README.md) describes each.

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
  one conformance suite. The engine orders opaque bytes; the logical
  key/value codecs that give those bytes meaning are owned by the
  path kernel (`marrow-kernel`), which is the engine's source-language consumer
  through a narrow byte seam.

### Compiler, image, verifier, VM, and path kernel

A small typed program travels the full production path. The storeless compiler
(`marrow-compile`) checks a growing subset and lowers to a reproducible program
image (`marrow-image`); it opens no store and cannot mint a verified image. The
independent verifier (`marrow-verify`) is the only image decoder and rejects a
malformed or hostile image in bounded phases — envelope, table closure,
per-function structure and types, call/effect closure with all-cycle rejection,
and transaction-flow validation — before sealing a `VerifiedImage`. The stack VM
(`marrow-vm`) runs only a sealed image, with source-mapped runtime faults under
private bounds. Durable operations pass the stub path kernel (`marrow-kernel`),
which resolves effective authority (verifier-derived demand intersected with a
deployment ceiling and an invocation grant, before the first engine call),
carries the durable operation algebra, and drives the ordered-byte engine over a
versioned store profile with an in-transaction commit witness.

`marrow run <export>` drives this path end to end for a storeless export. A
durable program — a keyed resource, a store root, its transactions, reads, and
bounded iteration — compiles, independently verifies, and completes its durable
identity, but durable execution is in the trough: T01's in-process store open
died at D00, so `marrow run` no longer opens a store and reports a durable export
with the typed `cli.durable_unsupported` outcome. Durable execution returns as
the ephemeral-memory preview (E01) and, for the persistent terminal path, over a
companion runner (F02b); the CLI never opens a store again. A store root is a
singleton (no key), a single-column keyed root, or a composite keyed tuple of up
to eight ordered columns; each key column is a scalar in the closed orderable
durable-key set (`int`, `string`, `bool`, `bytes`, `date`, `instant`). Every
root — and each of its key columns — is a distinct durable graph node with a
complete entropy-minted identity in the committed machine-written `marrow.ids`
ledger (minted by `marrow run`, required by every path, tombstoned on
retirement), and the program's durable graph carries a stable 32-byte
durable-contract identity computed over those ledger ids and the graph shape
(including key-column order) — so an anchor move preserves durable identity (the
ledger-model property; a rename becomes an anchor move under the future apply
action, while the additive-only `run` mint does not) — which the verifier
independently recomputes from the image and rejects on mismatch. The
compiler fully lowers operations over the single-column keyed root; singleton,
composite-key, group/branch, and widened-field roots declare and verify their
identity but their operations are not yet lowered. The admitted subset is narrow
and grows lane by lane; a well-formed construct outside it is a typed
`check.unsupported` diagnostic.

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
third-party packages, executable/store binding,
online schema evolution, logical backup and restore, a supported packaged
desktop application, public path publication, a supported served profile,
concurrent multi-writer deployment, replication, high availability, signed
releases, or institutional protocol/compliance evidence. The compiler, program
image, verifier, VM, and path kernel are present but early: their admitted
language subset is narrow and their durable identity, lifecycle, and authority
attenuation are stubs with named refounding points.

## Current trust boundaries

- Filesystem permissions and the host process protect local store files.
- Checksums and structural checks detect selected corruption; they do not
  authenticate hostile storage or prove full application validity.
- Encryption at rest is delegated to the filesystem or substrate.
- TLS, authentication, identity providers, operator credentials, and hardware
  durability are deployment responsibilities.
- Static checking cannot establish application intent, correct policy design,
  regulatory compliance, or absence of external side channels.
