# Schema Compilation

`marrow-schema` is the first semantic pass after parsing. It lowers one parsed declaration — `ResourceDecl`, `EnumDecl`, or `StoreDecl` — into a typed tree shape that downstream crates pattern-match instead of re-reading source spellings. It decides only what a single declaration can decide alone; cross-declaration name resolution and full type validation belong to `marrow-check`.

Every `compile_*` entry returns the schema **and** a `Vec<SchemaError>` together: compilation is best-effort, so a shape always comes back even on error and the checker keeps going.

## The shapes

- **Resource** — `compile_resource` / `compile_stored_resource` build a `ResourceSchema`: a source-ordered flat `Vec<Node>`. A `Node` is either a `Slot` (plain field or keyed leaf) or a `Group` (nested members). Keyed-ness is structural, not a flag — empty `key_params` means plain, non-empty means a keyed leaf. Sugar desugars to canonical keyed leaves so downstream paths are identical: `name: sequence[T]` becomes `name(pos: int): T`, `name: map[K,V]` becomes `name(key: K): V`.
- **Store** — `compile_store` builds a `StoreSchema` (durable root, identity keys, indexes) over a compiled `ResourceSchema`. `SavedRootSchema::single_int_root` is the single-int-root `nextId` policy gate the checker types `nextId(^root)` against; the runtime re-checks the same contract on the lowered place via its own `single_int_identity`. `next_id_shape` is the shared rejection-message wording — single-sourced in the checker and reused verbatim by the runtime, so both report the same shape.
- **Enum** — `compile_enum` builds an `EnumSchema`: members flattened pre-order DFS with parent links. Traversal indices are source-order positions, **not** durable value identity — identity lives in the parent-link tree shape.

## Rules it owns

| Rule | Code |
|---|---|
| Saved key must be an orderable scalar (every scalar but `decimal`) | `SCHEMA_UNORDERABLE_KEY` |
| Identity/named/sequence/unknown can't be a key | `SCHEMA_NONSCALAR_KEY` |
| `unknown` forbidden anywhere inside a managed saved schema | (rejected; local resources exempt) |
| `map[K,V]` only as unkeyed, unrequired field; any other `map[...]` rejected | `SCHEMA_UNSUPPORTED_TYPE` |
| Enum category must have children | `SCHEMA_CATEGORY_LEAF` |
| Non-category enum parent forbidden | `SCHEMA_PARENT_NOT_CATEGORY` |
| Index requires a keyed root; non-unique index must end with all identity keys in declaration order | `SCHEMA_INDEX_REQUIRES_KEYED_ROOT` |
| Duplicate member/key, key-member collision, index collisions | `SchemaNameCollision` payloads |

`classify_key_type` is purely structural — no enum/resource list. Index args diverge in exactly one way: a `Named` arg (an enum the checker later proves scalar) is accepted where a written key would reject it. The category⟺has-children lockstep makes a value-position reject cover exactly the categories and `match` cover exactly the childless non-categories, so a legal-but-uncoverable value is impossible.

`SchemaError` messages are render-only. The asserted contract is the `SchemaErrorKind` payload (typed target enums: `SchemaDuplicateTarget`, `SchemaSavedUnknownTarget`, `SchemaKeyTarget`, `SchemaNameCollision`) and the stable `schema.*` codes — tests match kinds and codes, never prose.

## Shared descriptor tables

Two single-source tables live here so checker and runtime never grow parallel copies:

- **`stdlib.rs`** — one `StdOp` row per `std::<module>::<op>`: param types, return type, and the runtime `Capability` family (`Pure`/`Clock`/`Env`/`Log`/`Io`/`Assert`). `lookup` types calls in the checker; the runtime dispatches off the same rows. An op absent from the table is unrecognized: its type stays `Unknown` and arg checking stays the runtime's job.
- **`error.rs`** — the one descriptor of the builtin `Error` shape (`code`/`message` required, `help`/`data` optional). `fields` / `field` serve both checker field-typing and runtime construction validation.

## Module map

| File | Responsibility |
|---|---|
| `crates/marrow-schema/src/lib.rs` | All schema compilation: `Type` resolution, `ResourceSchema`/`StoreSchema`/`EnumSchema`/`Node` types, `compile_*` entries, saved-data rule checks, sequence/map desugaring, `SchemaError` vocabulary |
| `crates/marrow-schema/src/stdlib.rs` | The `std::<module>::<op>` descriptor table (`StdOp`, `Capability`) with `lookup` and `all` |
| `crates/marrow-schema/src/error.rs` | The builtin `Error` shape (`ErrorField`, `FIELDS`) with `fields` and `field` |

## Type resolution

`Type::resolve` is total and module-blind: every source spelling maps to exactly one `Type` variant (`Scalar`/`Sequence`/`Identity`/`Named`/`Unknown`). Anything undecidable from text alone — a bare or qualified name — lands in `Type::Named` for the checker to promote to a resource/enum reference or reject. The canonical `ScalarType` vocabulary is re-exported from `marrow-store::value`; this crate pulls `marrow-store` with default features only (codec, not the redb backend).

## Read next

- `compile_resource` / `compile_stored_resource` (`lib.rs`) — the resource lowering split and why two entries exist (saved-`unknown` rejection, `map` sugar).
- `member_node` / `sequence_leaf` / `map_leaf` / `MapLeaf` (`lib.rs`) — the single owner of member→`Node` lowering and collection desugaring.
- `classify_key_type` / `index_arg_type_key_error` (`lib.rs`) — the orderable-scalar allowlist and the one index-arg divergence.
- `compile_store` index checks (`lib.rs`) — keyed-root requirement and the trailing-identity-key rule for non-unique indexes.
- `EnumSchema::walk_member_path` / `flatten_enum_members` (`lib.rs`) — the shared value/`is`/`match` path walk and category⟺has-children enforcement.
- `SavedRootSchema::single_int_root` (`lib.rs`) — the checker's single-int-root `nextId` gate (the runtime re-checks the same contract via `single_int_identity` in `marrow-run`); `next_id_shape` is the matching rejection-message helper, single-sourced and reused verbatim by the runtime.
- `TABLE` / `StdOp` / `lookup` (`stdlib.rs`) — the row shape that keeps std typing and dispatch single-sourced.
