# Project Status

Marrow is experimental, unreleased software. This page separates current
implementation from architectural direction. It is descriptive; exact current
language behavior remains in the [Language Reference](language/).

On the main branch, “current” means the implementation at the same Git revision
as this page. A release snapshot must identify its release and source revision.

## Status Categories

| Category | Meaning |
|---|---|
| Current | Implemented and part of the supported repository behavior |
| Legacy | Implemented, but not part of the intended architecture |
| Designed | Recorded direction whose detailed contract is not yet current |
| Accepted target | Explicitly approved unimplemented contract in `docs/design/` |
| Research | An open question, not an accepted contract |

## Current

### Language and checking

- Native `.mw` parser and formatter.
- Static types for scalars, resources, enums, sequences, keyed trees, and entry
  identities.
- Modules, functions, control flow, structured errors, presence checking, and
  host-capability boundaries.
- Local values and durable places reuse resource member types and path syntax;
  durable places add presence, keyed-child, transaction, and storage rules.
- Direct durable reads, assignments, deletion, and key iteration.
- Lexical transactions with atomic durable commit and rollback.
- Declared indexes maintained with managed writes.

### Execution and storage

- A checked executable representation consumed by a tree-walking interpreter.
- Memory and redb-backed implementations of the current ordered-tree contract.
- One owning write-capable process or session for the native store; the current
  native profile excludes read-only opens while a writer is open.
- Typed backup and restore, data inspection, integrity checking, and recovery
  commands.
- Linux and macOS source builds.

### Durable identity and evolution

- Accepted declaration identities distinct from current source spelling and
  represented by current catalog metadata; that representation is not the final
  path-graph contract.
- Detection of supported declaration changes against accepted declaration
  identities.
- Preview, state-bound witness, and apply workflows for supported evolutions.
- Index rebuild and selected data transforms as part of managed evolution.

Current evolution preview and apply are narrower than the designed generalized
program-image admission and activation contract.

### Tooling

- `check`, `run`, `test`, `fmt`, `data`, `evolve`, `backup`, `restore`, and
  related project commands.
- Typed diagnostic codes and machine-readable output.
- An implementation map for the Rust workspace.
- A downstream language-server repository consuming Marrow compiler APIs.

## Legacy Architecture Under Reconsideration

The following mechanisms exist in the repository and remain documented where
needed to describe current behavior. They are not long-term design commitments:

- `surface` declarations that repeat selected store fields and operations;
- generated create, update, delete, collection, read, and action operation
  families;
- opaque operation-tag HTTP routes and their current TypeScript client;
- the user-facing storage cost model and hidden-scan terminology;
- application sessions built directly around the current surface model.

New work should not expand or stabilize these concepts. Removing their
reference pages must occur with the implementation replacement so the current
documentation does not become false.

The repository also contains an experimental remote HTTP profile. Bearer
authentication is not compiler-integrated path authorization. The profile is
not a basis for a production security claim.

Earlier unimplemented proposals for per-operation and record-shaped
authorization are retired rather than designed direction. They remain in the
repository only until the documentation inventory classifies and removes or
rewrites them.

## Designed Direction

- A versioned immutable compiled program image with an explicitly documented
  execution target, plus a store-held active-image binding changed only by
  activation.
- Read-only store admission followed, when a transition is required, by atomic
  activation of data, accepted schema state, and the active-image binding.
- One compiler-owned semantic path graph with distinct schema path identities,
  entry-identity types, store UIDs, source spellings, concrete addresses carrying
  typed entry-key values, URI and authority projections, graph-version
  relations, and physical encodings.
- Module ownership and transitive effects over durable paths.
- A single authorized path kernel as the only logical durable access seam, with
  physical substrate recovery isolated beneath it as a named trusted component.
- Explicitly published URI address spaces and ordinary function bindings.
- Typed principals and path capabilities whose construction and delegation are
  restricted to named trusted runtime components.
- Embedded and served runtime profiles implementing one reference transition
  semantics and a declared isolation contract.
- Store-admission reports for consequential changes to schema identity,
  populated data, public paths, authority, and bindings.
- Official generated UI bindings and a local application development profile.

These items are direction, not current `.mw` syntax or supported runtime
behavior. See [Vision](vision.md).

## Accepted Target Contracts

There are no accepted target contracts after the documentation reset. The
[target-contract lifecycle](design/) defines how an exact unimplemented rule may
be approved without entering the current language reference.

## Not Implemented

- Bytecode, JIT, or native-code generation.
- Signed prebuilt releases or a one-line installer.
- A supported Electron or desktop application package.
- Compiler-integrated, runtime-enforced path authorization.
- A path-native public URI boundary.
- Multiple production storage substrates with one conformance claim.
- Concurrent multi-writer or high-availability service deployment.
- Replication, failover, or rolling mixed-version activation.
- FHIR or other institutional protocol conformance.

## Current Trust Boundaries

- Filesystem permissions and the host process protect local store files.
- Checksums detect selected accidental corruption; they are not authentication
  or tamper protection.
- Encryption at rest is delegated to the filesystem or storage substrate.
- Authentication, TLS, identity-provider behavior, operator credentials, and
  hardware durability remain operational assumptions.
- Static checking does not establish application intent, correct policy design,
  regulatory compliance, or freedom from external side channels.

See [Stability Contract](stability.md), [Operations](operations.md), and
[Backend Contract](backend-contract.md) for current detailed behavior.
