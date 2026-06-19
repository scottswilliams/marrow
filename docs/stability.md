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
| Accepted catalog | `marrow.catalog.json` is a versioned ABI for durable identity: catalog epoch, digest, entries, stable IDs, aliases, lifecycle, and accepted shape fields. Future id-allocation policy uses catalog evolution, not ad hoc fields accepted by v0.1 readers. |
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

The `surface` read foundation is active but not yet a stable transport or
generated-client contract. Checked surface facts are compiler facts over stores,
fields, indexes, read operations, footprints, and projections. Stable read
operations have accepted-catalog descriptors and operation tags; the active JSON
DTOs decode checked read request parameters through admitted runtime reads and
render already-executed surface reads with typed cursor-boundary JSON.
Linked-Rust embedding remains an implementation profile for hosting surface
facts and run sessions, not a separate app-data contract. HTTP serving, opaque
cursor tokens, generated clients, and write-body decode remain future profiles.
Until serving profiles ship, typed entry invocation (`marrow run` with `--arg`
and `--format json`) is the supported integration surface.

The `signature_digest` field in the run JSON envelope is reserved for the future
function ABI identity model and remains `null` in v0.1. The `raises` field is
reserved for the future declared error surface and remains `null` in v0.1.

A typed JSONL export remains a gated future surface. It depends on the designed
export boundary and type-identity contract; it is not a v0.1 data export API.

The egress-regime table lives in [operations.md](operations.md#egress-regimes).
That table is the single home for classifying application, tooling, admin,
portable-data, and source-tree output surfaces.
