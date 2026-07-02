# Stability Contract

This page names the v0.1 surfaces Marrow treats as release contracts.

## Platform And Distribution

Marrow v0.1 supports Unix targets only: Linux and macOS. Non-Unix builds are
outside the v0.1 contract; the stable-id entropy backstop on non-Unix platforms
may panic rather than report a Marrow diagnostic.

The v0.1.0 distribution channel is a tagged source release plus source install:

```sh
cargo install --locked --path crates/marrow
```

Prebuilt binaries and crates.io publication are post-v0.1 fast-follow channels,
not v0.1 release channels.

## Version And Engine Profile

`marrow --version` prints the binary version and current engine-profile tuple:

```console
$ marrow --version
marrow 0.1.0 engine-profile=(key=v0, layout-epoch=0, digest=77944eb86c08b665)
```

LayoutEpoch 0 is frozen for v0.1. A later physical byte-format change is a
LayoutEpoch recompile, never an in-place edit of v0.1 store bytes.

## Stable Surfaces

| Surface | v0.1 contract |
|---|---|
| CLI exit codes | `0` means success, `1` means a recoverable command failure, and `2` means command-line usage failed before the command body ran. |
| Dotted error codes | Lowercase dotted codes such as `parse.syntax`, `run.store_evolved`, and `restore.format_version` are stable machine codes. Message prose is render-only. |
| Error envelope | Machine-readable error reports carry the stable fields documented in [error-codes.md](error-codes.md): `code`, `kind`, optional `message`, optional `help`, optional `source_span`, and optional `data`. |
| `marrow.json` | The project configuration keys and validation rules in [project-config.md](project-config.md) are the v0.1 configuration ABI. There are no command-line storage overrides. |
| Saved-data identity | The live store is the authority for accepted saved-data identity, projected to the committed `marrow.lock` documented in [data-evolution.md](data-evolution.md). The lock's per-entry stable IDs, lifecycle, canonical paths with aliases, and shape fingerprints, plus its append-only ledger of retired and reserved IDs and its per-root activation epochs, are the v0.1 identity ABI. Future id-allocation policy flows through shape evolution, not ad hoc fields accepted by v0.1 readers. |
| Backup format | Backup `format_version` 6 is the portable backup/restore format. It carries manifest facts, accepted catalog rows, and typed data cells; generated indexes are rebuilt on restore. |
| Backup lineage field | `parent_snapshot_digest` is reserved and semantics-undefined in v0.1. Writers emit the empty sentinel, and readers reject a non-empty value. |
| Tree-cell interchange | The v0 tree-cell key/value codecs and backup cell stream are the single data interchange for v0.1. Raw store files, data dumps, traces, and dry-run reports are not sibling interchange formats. |

The native cost law counts one durable commit fsync per committed source
transaction; fresh native-store creation also syncs the containing directory.
The evolution staging contract is constant-memory for the staging path. These
are law-backed release contract statements, not latency or peak-memory benchmark
claims.

## Not Stable In v0.1

Raw native-store bytes are not stable across recompiles or engine-profile
changes. Move data through typed backup/restore when portability is required.

Human-readable message prose is not stable. Clients consume dotted codes, typed
JSON fields, store stamps, catalog IDs, and other structured facts.

The nine Rust crate APIs in this workspace (`marrow`, `marrow-catalog`,
`marrow-check`, `marrow-json`, `marrow-project`, `marrow-run`,
`marrow-schema`, `marrow-store`, and `marrow-syntax`) are unstable in v0.1.
`marrow-lsp` is a coordinated consumer of Marrow semantics, not proof of a
public Rust API. A future embedding contract is one facade surface, never direct
stability for the internal crates.

## Application Surfaces

The `surface` foundation is active but not yet a stable remote transport or
stable generated-client contract. Checked surface facts are compiler facts over
stores, fields, indexes, read operations, footprints, projections, sparse update
fields, create fields, delete operations, declared public actions, and declared
public read-only computed reads. Stable reads, computed reads, creates, sparse
updates, deletes, and actions have accepted-catalog descriptors and operation
tags; action tags reuse `entry.invoke.v1` identity over parameters and return
shape. `marrow check --format json|jsonl` exports the current surface ABI
descriptor set for successful checks. The active JSON DTOs decode checked read
request parameters through admitted runtime reads, decode computed-read
arguments through `entry.invoke.v1`, decode generated write request bodies
through admitted runtime create/update/delete handles, decode action arguments
through `entry.invoke.v1`, and render already-executed surface reads, computed
reads, creates, and action results with accepted-catalog typed JSON. Read and
computed-read DTOs execute over `ProjectSurfaceReadSession`, and
point/singleton create/update/delete plus action DTOs execute over
`ProjectSurfaceSession`, without exposing backing store handles. The successful
check JSON output also includes route-manifest rows derived from the exported
descriptors; those rows name JSON `POST` operation-tag paths and render aliases,
but they do not make aliases operation identity. The current serving profile is
`marrow serve`: loopback-bound, JSON-only, optional exact loopback CORS with
`--cors-origin`, at most one processed request per connection, backed by
`ProjectSurfaceReadSession` in default read-only mode for v1 read routes,
including computed reads and ranged index page reads, and backed by
`ProjectSurfaceSession` for v1 create/update/delete/action routes when `--write`
is passed.
`marrow client typescript` is the current
generated-client profile: it renders a self-contained TypeScript wrapper over
the same ABI, route manifest, and operation envelope without opening the store.
`marrow-run::ProjectSurfaceReadSession` is an unstable linked-Rust
implementation profile for read serving over an already accepted native store:
it opens the store read-only, fences drift, and exposes admitted surface reads
and computed reads by operation tag. `marrow-run::ProjectSurfaceSession` is the
matching unstable linked-Rust implementation profile for read/write surface
execution over an existing accepted native store: it opens the store writable,
requires store UID and commit metadata, fences drift without hidden repair, and
exposes admitted surface reads, computed reads, creates, sparse updates,
deletes, and actions by operation tag. It is a single-owner, sequential session;
while it is open, the native writer lock makes it the owning process/session for
these reads and writes and excludes another writer or read-only inspection.
Linked-Rust embedding remains an implementation profile for hosting surface
facts, run sessions, and these project surface slices, not a stable app-data
contract. Computed reads always run with a zero-capability host, and
host-effecting computed reads are rejected by the checker. The default project
operation envelope helper also runs actions with a zero-capability host; callers
that need action host capabilities use the explicit-host helper. The shipped
HTTP profile is `marrow serve` for loopback operation envelopes, plus the
explicit remote authenticated profile and its optional opaque cursor-token
transport profile. Linked-Rust surface
helpers, route manifest rows, and typed entry invocation remain implementation
profiles.
The linked-Rust entry descriptor profile is an unstable implementation surface:
`marrow-check` owns `entry.invoke.v1` descriptor tags over public entry
signatures, parameter shapes, accepted catalog identities, return presence, and
return shape, while `marrow-run` admits `EntryInvocation` values by checking the
callable ABI tag and read-only checked-program context before execution. It does
not make the Rust crates stable, does not define JSON request bodies, and does
not publish HTTP routes or generated-client names.

The run JSON envelope is the v0.1 command profile for entry results and faults:
success reports `result`, `output`, and an empty `diagnostics` array, while
faults report `diagnostics` plus captured `output`. Entry ABI identity remains a
linked-Rust implementation profile in v0.1 and is not exposed as a separate run
JSON signature field. Declared error surfaces remain future work.

A typed JSONL export remains a gated future surface. It depends on the designed
export boundary and type-identity contract; it is not a v0.1 data export API.

The egress-regime table lives in [operations.md](operations.md#egress-regimes).
That table is the single home for classifying application, tooling, admin,
portable-data, and source-tree output surfaces.
