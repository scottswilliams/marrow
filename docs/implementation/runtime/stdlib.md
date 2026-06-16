# Standard library boundary

The runtime evaluates a checked `std::<module>::<op>` call or a language builtin
(`print`, `count`, `exists`, conversions, `Error(...)`, index lookup) into a
`Value` or a host effect. The language-facing standard-library contract lives in
`docs/language/standard-library.md`; this page maps the runtime boundary. The
checker has already resolved each call to a typed target: `CheckedStdCall`
carrying `requires_capability: Option<Capability>`, or a `CheckedBuiltinCall` /
`CheckedCallTarget` variant, against the single descriptor table in
`crates/marrow-schema/src/stdlib.rs`. This boundary never re-parses op-name
strings to decide arity, types, or which capability is needed; it branches on
the typed kind.

Dispatch enters from `eval_std_call` and `eval_builtin_call` in `crates/marrow-run/src/call.rs`. `eval_std_call` matches `requires_capability`: `Some(Clock)`, `Some(Context)`, `Some(Environment)`, `Some(Log)`, and `Some(Filesystem)` route to host-effect handlers; `None` routes to `eval_assert` for module `assert` and otherwise to `eval_std`. Pure helpers compute in place and never touch `env.host`. Host-effect helpers read their capability off `Env`'s `Host` — `clock`/`context`/`environment`/`log` are `Option` fields, and filesystem access is the `bool` `Host.filesystem` flag — and raise `run.capability` when it is absent.

## The two halves

- **No capability** (`std_pure.rs`, focused `std_*` modules, plus `args`/`assertions`/`conversion`/`count`/`error_constructor`/`index_lookup`/`math`/`output`): text/math/bytes/clock-format/parse, scalar text readers/builders, deterministic helpers, conversions, counting, `std::assert`, the `Error(...)` constructor, and index lookups. Clock format/parse helpers are pure; only `now`/`today` need the `Clock` capability.
- **Host effects** (`host_effects.rs`): `clock` now/today, `context`, `env`, `log`, `io`. Each pulls its capability off `env.host` (`clock`/`context`/`environment`/`log` are `Option`s; `io` is the `bool` `filesystem` flag). Writes (`std::io::write*`, `print`, `std::log`) call `env.guard_rollback_sensitive_host_effect` before touching the outside world, rejecting them with `run.capability` when transaction depth > 0 — external effects cannot be rolled back. Reads (io read, env, clock, context) are unguarded.

## Module map

| File | Responsibility |
| --- | --- |
| `crates/marrow-run/src/stdlib.rs` | Builtin-support root: declares the shared args/assertion/conversion/count/error/index/math/output helpers and re-exports their entry points to the crate. |
| `crates/marrow-run/src/std_pure.rs` | Pure dispatch: `eval_std` routes the module string to text/math/bytes/clock(format+parse) handlers and focused scalar stdlib modules. |
| `crates/marrow-run/src/std_json.rs`, `std_csv.rs`, `std_id.rs`, `std_random.rs`, `std_audit.rs`, `std_error_helpers.rs`, `std_matrix.rs` | Scalar-only stdlib extensions that return existing `Value` scalars without adding opaque runtime value types. |
| `crates/marrow-run/src/host_effects.rs` | Capability handlers `eval_clock_capability`/`eval_context`/`eval_env`/`eval_log`/`eval_io`; capability gating and rollback-sensitive write guarding. |
| `crates/marrow-run/src/stdlib/args.rs` | Typed arg evaluators: `eval_typed_arg` plus `eval_bytes`/`decimal`/`instant`/`date`/`duration`/`text_arg` coerce one `ExecArg` to a concrete `Value`. |
| `crates/marrow-run/src/stdlib/assertions.rs` | `std::assert`: `isTrue`/`isFalse`/`absent`/`fail`; raises `run.assert` on failure, returns `None` on success. |
| `crates/marrow-run/src/stdlib/conversion.rs` | Scalar/ErrorCode/bytes conversions driven by `ConversionKind`; parses via store `decode_value`/`Decimal`, splitting decimal overflow from malformed text and validating ErrorCode text through `marrow_schema::error`. |
| `crates/marrow-run/src/stdlib/count.rs` | `count`/`exists` over saved paths, local collections, and typed maybe-present call results; routes through specialized counters before falling back to a store child-count. |
| `crates/marrow-run/src/stdlib/error_constructor.rs` | `Error(...)`: validates named args and `code` text against `marrow_schema::error`, then builds a `Value::Resource` of `(name, value)` fields. |
| `crates/marrow-run/src/stdlib/index_lookup.rs` | Unique-index lookup: resolves a checked `Index` terminal to an `IndexAddress`, scans, decodes the payload to an identity, answers presence/count. |
| `crates/marrow-run/src/stdlib/math.rs` | Integer `int_remainder` (shared with the `%` remainder operator lowering) and `int_modulo` (backs `std::math::modulo` only); divide-by-zero/overflow faults, sign from divisor. |
| `crates/marrow-run/src/stdlib/output.rs` | `print`: renders one runtime value, guards the write, appends a newline to `env.output` (not the host log sink). |
| `crates/marrow-run/src/stdlib/tests.rs` | `every_table_row_reaches_a_live_handler`: every `marrow_schema::stdlib::all()` row routes to a handler that does not return `run.unsupported`. |

## Invariants worth knowing

- The `marrow_schema::stdlib` `TABLE` is the single source of truth for arity, param/return types, result presence, and optional host capability. A new std helper is one row, not parallel checker + runtime entries. `unreachable!()` arms in `host_effects.rs` encode that the checker already filtered op names against this table.
- `ConversionKind` carries the checker's resolved conversion decision so the runtime never re-derives it from a name. `conversion.rs` marks `Conversion(ScalarType::Bytes)` unreachable because bytes resolves to its own `CheckedBuiltinCall::Bytes`.
- `print` appends to `env.output` (the run's stdout buffer); `std::log` appends to the separate `host.log` sink. Output is always available; log requires the capability.
- Conversion error taxonomy is owned downstream: `convert_to_decimal` defers overflow-vs-malformed to `marrow-store`'s `Decimal` parser.
- Unique-index reads decode the stored payload into an identity of the expected arity and raise one canonical `run.type` corruption fault (`decode_unique_index_identity`); presence/count comes from `ExactUniqueIndexLookupValue` without materializing the record. `keys(...)` over a unique index is unsupported (`check_key_collection`).
- `exists(maybe_call())` is intentionally call-expression-scoped: it evaluates the checked maybe-present call once and maps catchable `run.absent_element` to `false`, without creating a durable saved-path proof.

## Read next

- `crates/marrow-run/src/call.rs` — `eval_std_call` / `eval_builtin_call`: the dispatch fan-out reaching every entry point.
- `crates/marrow-schema/src/stdlib.rs` — `TABLE` / `Capability` / `StdOp`: the single descriptor table the whole boundary is organized around.
- `crates/marrow-run/src/host_effects.rs` — `eval_io` / `IoOp` / `eval_log`: capability gating, rollback-sensitive guarding, and the read/write error-code split.
- `crates/marrow-run/src/stdlib/index_lookup.rs` — `read_exact_unique_index_lookup_value` / `decode_unique_index_identity`: the densest store-touching logic, identity decode and the presence/count contract.
