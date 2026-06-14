# Schema Compilation

`marrow-schema` is the first semantic pass after parsing. It lowers one parsed declaration — `ResourceDecl`, `EnumDecl`, or `StoreDecl` — into a typed tree shape that downstream crates pattern-match instead of re-reading source spellings. It decides only what a single declaration can decide alone; cross-declaration name resolution and full type validation belong to `marrow-check`.

Every `compile_*` entry returns the schema **and** a `Vec<SchemaError>` together: compilation is best-effort, so a shape always comes back even on error and the checker keeps going.

## The shapes

- **Resource** — `compile_resource` / `compile_stored_resource` build a `ResourceSchema`: a source-ordered flat `Vec<Node>`. A `Node` is either a `Slot` (plain field or keyed leaf) or a `Group` (nested members). Keyed-ness is structural, not a flag — empty `key_params` means plain, non-empty means a keyed layer. Sequence sugar desugars to the canonical keyed leaf: `name: sequence[T]` becomes `name(pos: int): T`. The checker later resolves an explicit keyed field whose value type names a resource into a keyed `Group` carrying that resource as its entry type; the checker also owns project-aware keyed enum, resource, and unknown-name resolution.
- **Store** — `compile_store` builds a `StoreSchema` (durable root, identity keys, indexes) over a compiled `ResourceSchema`. `StoreSchema::single_int_root` is the single-int-root `nextId` policy gate the checker types `nextId(^root)` against; the runtime re-checks the same contract on the lowered place via its own `single_int_identity`. `next_id_shape` is the shared rejection-message wording — single-sourced in the checker and reused verbatim by the runtime, so both report the same shape.
- **Enum** — `compile_enum` builds an `EnumSchema`: members flattened pre-order DFS with parent links. Traversal indices are source-order positions, **not** durable value identity — identity lives in the parent-link tree shape.

## Rules it owns

| Rule | Code |
|---|---|
| Saved key must be an orderable scalar (every scalar but `decimal`) | `SCHEMA_UNORDERABLE_KEY` |
| Identity/named/sequence/unknown can't be a key | `SCHEMA_NONSCALAR_KEY` |
| `unknown` forbidden anywhere inside a managed saved schema | (rejected; local resources exempt) |
| Plain saved-field named values must be locally enum-shaped; project-aware keyed named value/resource resolution belongs to `marrow-check::keyed_entries` | `SCHEMA_NON_ENUM_NAMED_FIELD` |
| Enum category must have children | `SCHEMA_CATEGORY_LEAF` |
| Non-category enum parent forbidden | `SCHEMA_PARENT_NOT_CATEGORY` |
| Index requires a keyed root; non-unique index must end with all identity keys in declaration order | `SCHEMA_INDEX_REQUIRES_KEYED_ROOT` |
| Duplicate member/key, key-member collision, index collisions | `SchemaNameCollision` payloads |

`classify_key_type` is purely structural — no enum/resource list. Index args diverge in exactly one way: a `Named` arg (an enum the checker later proves scalar) is accepted where a written key would reject it. The category⟺has-children lockstep makes a value-position reject cover exactly the categories and `match` cover exactly the childless non-categories, so a legal-but-uncoverable value is impossible.

`SchemaError` messages are render-only. The asserted contract is the `SchemaErrorKind` payload (typed target enums: `SchemaDuplicateTarget`, `SchemaSavedUnknownTarget`, `SchemaKeyTarget`, `SchemaNameCollision`) and the stable `schema.*` codes — tests match kinds and codes, never prose.

## Shared descriptor tables

Two single-source tables live here so checker and runtime never grow parallel copies:

- **`presence.rs`** — the shared `ReturnPresence` marker (`Always` or `MaybePresent`) used by stdlib descriptors and checked user-function descriptors. It is not tied to stdlib; it is the one typed result-presence vocabulary exported by the schema crate.
- **`stdlib.rs`** — one `StdOp` row per `std::<module>::<op>`: param types, return type, result presence, and an optional runtime `Capability` (`Clock`, `Environment`, `Log`, `Filesystem`, or `Maintenance`). `lookup` types calls in the checker; the runtime dispatches off the same rows. Rows with no capability are pure except for `std::assert`, which routes to the assert handler by module name. Calls under a known std module that are absent from the table are checker errors, not runtime extension hooks.
- **`error.rs`** — the one descriptor of the builtin `Error` shape (`code`/`message` required, `help`/`data` optional) plus the shared error-code grammar. `fields` / `field` serve checker field-typing and runtime construction validation; `is_error_code_text` serves `ErrorCode` conversion and `Error.code`.

## Module map

| File | Responsibility |
|---|---|
| `crates/marrow-schema/src/lib.rs` | Thin crate root: module declarations and the `pub use` re-exports that fix the public API paths |
| `crates/marrow-schema/src/types.rs` | `Type` resolution and the `ResourceSchema`/`StoreSchema`/`Node`/`NodeKind`/`KeyDef`/`IndexSchema` tree shapes with their query impls |
| `crates/marrow-schema/src/enums.rs` | `EnumSchema`/`EnumMemberSchema`/`MemberPathResolution` and the value/`is`/`match` member-path queries |
| `crates/marrow-schema/src/errors.rs` | The `SchemaError` vocabulary: `SchemaErrorKind`, the typed target enums, the `schema.*` codes, and the message constructors |
| `crates/marrow-schema/src/compile.rs` | The `compile_*` entries, member→`Node` lowering with sequence desugaring, enum flattening, and `sequence` type-spelling parsing |
| `crates/marrow-schema/src/validate.rs` | Single-declaration validation: duplicate-name tracking, the orderable-scalar key allowlist, store identity-key/index checks, and the saved-member rules |
| `crates/marrow-schema/src/presence.rs` | The shared return-presence enum used by stdlib rows and checked user-function descriptors |
| `crates/marrow-schema/src/stdlib.rs` | The `std::<module>::<op>` descriptor table (`StdOp`, `Capability`) with `lookup` and `all` |
| `crates/marrow-schema/src/error.rs` | The builtin `Error` shape (`ErrorField`, `FIELDS`) with `fields`, `field`, and `is_error_code_text` |

## Type resolution

`Type::resolve` is total and module-blind: every source spelling maps to exactly one `Type` variant (`Scalar`/`Sequence`/`Identity`/`Named`/`Unknown`). Anything undecidable from text alone — a bare or qualified name — lands in `Type::Named` for the checker to promote to a resource/enum reference or reject. The canonical `ScalarType` vocabulary is re-exported from `marrow-store::value`; this crate pulls `marrow-store` with default features only (codec, not the redb backend).

## Read next

- `compile_resource` / `compile_stored_resource` (`compile.rs`) — the resource lowering split and why two entries exist (saved-`unknown` rejection).
- `member_node` / `sequence_leaf` (`compile.rs`) — the single owner of member→`Node` lowering and sequence desugaring.
- `classify_key_type` / `index_arg_type_key_error` (`validate.rs`) — the orderable-scalar allowlist and the one index-arg divergence.
- `compile_store` (`compile.rs`) / `check_store_index` (`validate.rs`) — keyed-root requirement and the trailing-identity-key rule for non-unique indexes.
- `EnumSchema::walk_member_path` (`enums.rs`) / `flatten_enum_members` (`compile.rs`) — the shared value/`is`/`match` path walk and category⟺has-children enforcement.
- `StoreSchema::single_int_root` (`types.rs`) — the checker's single-int-root `nextId` gate (the runtime re-checks the same contract via `single_int_identity` in `marrow-run`); `next_id_shape` is the matching rejection-message helper, single-sourced and reused verbatim by the runtime.
- `TABLE` / `StdOp` / `lookup` (`stdlib.rs`) — the row shape that keeps std typing and dispatch single-sourced.
