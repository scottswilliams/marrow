# Schema Compilation

`marrow-schema` is the first semantic pass after parsing. It lowers one parsed declaration — `ResourceDecl`, `EnumDecl`, or `StoreDecl` — into a typed tree shape that downstream crates pattern-match instead of re-reading source spellings. It decides only what a single declaration can decide alone; cross-declaration name resolution and full type validation belong to `marrow-check`.

Every `compile_*` entry returns the schema **and** a `Vec<SchemaError>` together: compilation is best-effort, so a shape always comes back even on error and the checker keeps going.

## The shapes

- **Resource** — `compile_resource` builds a `ResourceSchema`: a source-ordered flat `Vec<Node>`. A `Node` is either a `Slot` (plain field or keyed leaf) or a `Group` (nested members). Keyed-ness is structural, not a flag — empty `key_params` means plain, non-empty means a keyed layer. A `Slot`'s `error_code` flag records the `ErrorCode` spelling, which resolves to a `Str` `Type` and so is otherwise lost; it lets a write into the place enforce the error-code grammar. Sequence sugar desugars to the canonical keyed leaf: `name: sequence[T]` becomes `name(pos: int): T`. The checker later resolves an explicit keyed field whose value type names a resource into a keyed `Group` carrying that resource as its entry type; the checker also owns project-aware keyed enum, resource, and unknown-name resolution.
- **Store** — `compile_store` builds a `StoreSchema` (durable root, identity keys, indexes) over a compiled `ResourceSchema`. `StoreSchema::single_int_root` is the single-int-root `nextId` policy gate the checker types `nextId(^root)` against; the runtime re-checks the same contract on the lowered place through `is_single_int_sequence` (`marrow-check`), the one predicate that classifies a single-int keyspace as a 1-based sequence for the checker, read guard, and write planner alike. `next_id_shape` is the shared rejection-message wording — single-sourced in the checker and reused verbatim by the runtime, so both report the same shape.
- **Enum** — `compile_enum` builds an `EnumSchema`: members flattened pre-order DFS with parent links. Traversal indices are source-order positions, **not** durable value identity — identity lives in the parent-link tree shape.

## Rules it owns

| Rule | Code |
|---|---|
| Saved key must be an orderable scalar (every scalar but `decimal`) | `SCHEMA_UNORDERABLE_KEY` |
| Identity/named/sequence/unknown/optional can't be a key | `SCHEMA_NONSCALAR_KEY` |
| `unknown` forbidden anywhere inside a managed saved schema | (rejected; local resources exempt) |
| Optional (`T?`) forbidden in a saved field/keyed-leaf value (a field is sparse by default; `?` is the code-level read type) | `SCHEMA_OPTIONAL_IN_SAVED` |
| Plain saved-field named values must be locally enum-shaped; project-aware keyed named value/resource resolution belongs to `marrow-check::keyed_entries` | `SCHEMA_NON_ENUM_NAMED_FIELD` |
| Enum category must have children | `SCHEMA_CATEGORY_LEAF` |
| Non-category enum parent forbidden | `SCHEMA_PARENT_NOT_CATEGORY` |
| Index requires a keyed root; non-unique index must end with all identity keys in declaration order | `SCHEMA_INDEX_REQUIRES_KEYED_ROOT` |
| Duplicate member/key, key-member collision, index collisions | `SchemaNameCollision` payloads |

`classify_key_type` is purely structural — no enum/resource list. `local_key_type_error` reuses it so a local keyed `var`/keyed parameter key (checked by `marrow-check`) obeys the same allowlist as a saved key. Index args diverge in exactly one way: a `Named` arg (an enum the checker later proves scalar) is accepted where a written key would reject it. The category⟺has-children lockstep makes a value-position reject cover exactly the categories and `match` cover exactly the childless non-categories, so a legal-but-uncoverable value is impossible.

`SchemaError` messages are render-only. The asserted contract is the `SchemaErrorKind` payload (typed target enums: `SchemaDuplicateTarget`, `SchemaSavedPosition` — the saved value position shared by the `unknown` and optional rejections — `SchemaKeyTarget`, `SchemaNameCollision`) and the stable `schema.*` codes — tests match kinds and codes, never prose.

## Shared descriptor tables

Two single-source tables live here so checker and runtime never grow parallel copies:

- **`stdlib.rs`** — one `StdOp` row per `std::<module>::<op>`: param types, return type, and an optional runtime `Capability` (`Clock`, `Context`, `Environment`, `Log`, or `Filesystem`). Presence lives in the return type: a maybe-present op carries an `OptionalScalar(T)` return (`ReturnType`), which the checker types as `T?`; there is no separate presence column. `lookup` types calls in the checker; the runtime dispatches off the same rows. Rows with no capability are pure except for `std::assert`, which routes to the assert handler by module name. Calls under a known std module that are absent from the table are checker errors, not runtime extension hooks.
- **`error.rs`** — the one descriptor of the builtin `Error` shape (`code`/`message` required, `help`/`data` optional) plus the shared error-code grammar. `field` serves checker field-typing; `fields` serves checker field-typing and runtime construction (slot layout); `is_error_code_text` is the single grammar owner the `ErrorCode(...)` conversion, the `Error.code` check, and every literal or dynamic value coerced into an `ErrorCode` place validate through.

## Module map

| File | Responsibility |
|---|---|
| `crates/marrow-schema/src/lib.rs` | Thin crate root: module declarations and the `pub use` re-exports that fix the public API paths |
| `crates/marrow-schema/src/types.rs` | `Type` resolution and the `ResourceSchema`/`StoreSchema`/`Node`/`NodeKind`/`KeyDef`/`IndexSchema` tree shapes with their lookup helpers |
| `crates/marrow-schema/src/enums.rs` | `EnumSchema`/`EnumMemberSchema`/`MemberPathResolution` and the value/`is`/`match` member-path lookups |
| `crates/marrow-schema/src/errors.rs` | The `SchemaError` vocabulary: `SchemaErrorKind`, the typed target enums, the `schema.*` codes, message constructors, and the store/index invalidation classifier consumed by checker backing validation |
| `crates/marrow-schema/src/compile.rs` | The `compile_*` entries, member→`Node` lowering with sequence desugaring, enum flattening, and `sequence` type-spelling parsing |
| `crates/marrow-schema/src/validate.rs` | Single-declaration validation: duplicate-name tracking, the orderable-scalar key allowlist, store identity-key/index checks, and the saved-member rules (`unknown`/optional value types) |
| `crates/marrow-schema/src/stdlib.rs` | The `std::<module>::<op>` descriptor table (`StdOp`, `Capability`) with `lookup` and `all` |
| `crates/marrow-schema/src/error.rs` | The builtin `Error` shape (`ErrorField`, `FIELDS`) with `fields`, `field`, and `is_error_code_text` |

## Type resolution

`Type::resolve` is total and module-blind: every source spelling maps to exactly one `Type` variant (`Scalar`/`Sequence`/`Identity`/`Named`/`Unknown`/`Optional`). Anything undecidable from text alone — a bare or qualified name — lands in `Type::Named` for the checker to promote to a resource/enum reference or reject. The canonical `ScalarType` vocabulary is re-exported from `marrow-store::value`; this crate pulls `marrow-store` with default features only (codec, not the redb backend).

A trailing `?` is the optional suffix: `resolve_text` strips one and wraps the base through `Type::optional`, the single flattening constructor (`optional(Optional(x)) == Optional(x)`), so an optional never nests by representation. `embeds_optional` mirrors `embeds_unknown` for the saved-shape walk. Optionality is a code-level type with **no durable footprint**, enforced at one structural choke-point: every value slot is built through `NodeKind::slot`, which asserts `!ty.embeds_optional()` — the single owner of "a durable leaf is never optional". `compile.rs` strips any rejected `?` (`without_optional`) before slot construction so the best-effort schema stays optional-free while the validator reports the source error. The store-side counterpart is a `debug_assert` at `marrow-store`'s `encode_value` boundary: a saved cell encodes one present scalar, the only cell discriminant, and absence is the lack of a cell rather than a null value.

## Read next

- `compile_resource` (`compile.rs`) — the one resource-lowering entry; saved-`unknown` and saved-optional rejection lives in `validate.rs`.
- `member_node` / `sequence_leaf` (`compile.rs`) — the single owner of member→`Node` lowering and sequence desugaring; routes every slot through `NodeKind::slot` (`types.rs`), the durable "never optional" choke-point.
- `ResourceSchema::node_at` (`types.rs`) — the one canonical saved-path-chain → terminal `Node` walk; `field_type` and the checker's field resolution route through it instead of re-deriving the descent. `leaf_type` keeps the stricter `descend_layers` walk because its terminal must itself be a keyed-leaf layer, not a plain field.
- `classify_key_type` / `index_arg_type_key_error` (`validate.rs`) — the orderable-scalar allowlist and the one index-arg divergence.
- `compile_store` (`compile.rs`) / `check_store_index` (`validate.rs`) — keyed-root requirement and the trailing-identity-key rule for non-unique indexes.
- `EnumSchema::walk_member_path` (`crates/marrow-schema/src/enums.rs`) /
  `flatten_enum_members` (`compile.rs`) — the shared value/`is`/`match` path
  walk and category⟺has-children enforcement.
- `StoreSchema::single_int_root` (`types.rs`) — the checker's single-int-root `nextId` gate (the runtime re-checks the same contract via the shared `is_single_int_sequence` predicate in `marrow-check`); `next_id_shape` is the matching rejection-message helper, single-sourced and reused verbatim by the runtime.
- `TABLE` / `StdOp` / `lookup` (`stdlib.rs`) — the row shape that keeps std typing and dispatch single-sourced.
