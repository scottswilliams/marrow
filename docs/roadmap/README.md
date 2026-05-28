# Marrow Roadmap

This roadmap tracks the work needed to make the implementation match the
language reference in [`../language/`](../language/) and the runtime design in
[`../implementation.md`](../implementation.md).

Marrow is a fresh product, but it is not a blank-room exercise. The old M Rust
codebase contains useful engineering lessons and a few reusable patterns. The
Marrow implementation should reuse only what fits the `.mw` language/database
model, and rewrite anything that would make Marrow inherit Classic M, globals,
routines, Postgres, or compatibility surfaces.

## Product Shape

The first real Marrow release presents:

- the `marrow` command;
- documented `.mw` syntax, formatting, checking, and runtime behavior;
- resources as typed trees for local and saved data;
- a small native project store;
- visible generated indexes;
- stable dotted errors and structured CLI output;
- typed and raw inspection tools;
- portable backup and restore;
- a clear backend contract proven by memory and native storage.

The release does not require a background service, a remote database, a broad
standard library, a web framework, alternate language modes, or bundled
database-specific adapters.

## Reuse Policy

Reuse from M Rust is allowed when the result still looks like Marrow:

| Keep | Use |
|---|---|
| CLI test patterns | Process-level tests for exit codes, stdout, stderr, and temp projects. |
| Diagnostic envelope ideas | Stable `code`, `kind`, `message`, `source_span`, optional `help`, and structured `data`. |
| Capability-profile pattern | Check storage promises by capability, not backend name. |
| Backend conformance style | Shared laws for ordering, presence, scans, deletes, roots, replay, and limits. |
| Native-store operating lessons | File layout, format version checks, locking, read-only inspection, and corruption reporting. |

Rewrite instead of copying:

- lexer, parser, syntax tree, checker, formatter, and runtime;
- Classic M ASTs, commands, routines, globals, special variables, and M error
  codes;
- `MValue`, Classic numeric coercion, and Classic collation;
- public storage names such as globals, routines, and routine roots;
- Postgres or any other database-specific adapter;
- compatibility paths that make `.mw` look like a mode inside another
  language.

The rule is simple: if code cannot be described naturally in the terms used by
the Marrow reference, it does not belong in the default Marrow repository.

## Quality Bar

A slice is ready only when a senior language or database implementer would see
a coherent boundary:

- the code names Marrow concepts directly;
- diagnostics use dotted Marrow error codes;
- source spans are preserved from the first parser slice onward;
- unsupported behavior is explicit and tested, not silently accepted;
- tests include real documented `.mw` examples;
- every public surface says what it actually does today;
- the implementation remains small enough to audit.

## Build Order

Build from source facts to saved data. Each step leaves one useful, tested
surface behind.

| Step | Surface | Proves |
|---|---|---|
| 1 | Bootstrap spine | The repo builds as Marrow and has a clear reuse boundary. |
| 2 | Source outline | Documented modules, resources, and functions parse into Marrow facts. |
| 3 | Full syntax | Statements and expressions parse with recovery and stable spans. |
| 4 | Formatter | Reference syntax round-trips before runtime work expands. |
| 5 | Schema model | Local and saved resources share one typed tree shape. |
| 6 | Saved-tree contract | Memory storage proves ordered paths, values, presence, scans, and limits. |
| 7 | Managed writes | Resource writes, deletes, merges, indexes, and failures stay coherent. |
| 8 | Runtime | Checked `.mw` functions run against local values and saved trees. |
| 9 | Native storage | Redb-backed native storage passes the same conformance suite. |
| 10 | Tools | Users can run, inspect, edit, back up, restore, and diagnose projects. |

Prefer one complete vertical path over several partial subsystems.

## 1. Bootstrap Spine

- Keep the repository Apache-2.0 only.
- Keep `docs/language/` as the source of truth for `.mw`.
- Keep `docs/implementation.md` as the source of truth for runtime, backend,
  server, and inspection architecture.
- Keep M Rust reuse at the pattern or small-utility level unless a file can be
  renamed and explained entirely as Marrow.
- Keep the default workspace free of Classic M, Postgres, globals, routines,
  and compatibility modes.

Done when a fresh checkout builds, `marrow --help` describes Marrow only, and
the roadmap clearly separates reuse from rewrite.

## 2. Source Outline

- Add `marrow-syntax` as the native `.mw` source crate.
- Parse modules, imports, constants, resources, saved roots, resource fields,
  keyed layers, indexes, documentation comments, stable IDs, functions,
  parameters, and return types.
- Preserve source spans on declarations and diagnostics.
- Reject tabs, `internal`, `private`, `proc`, and other obsolete surface
  syntax with dotted Marrow errors.
- Wire `marrow check` to source parsing without claiming semantic checking.
- Report text, JSON, and JSONL diagnostics through the Marrow error envelope.
- Parse the reference sample in [`../language/sample.md`](../language/sample.md).

Done when `marrow check` can parse a documented resource module, report syntax
errors with `parse.syntax`, and exit with the documented CLI codes.

## 3. Full Syntax

- Replace the outline parser with a full token stream and syntax tree.
- Parse indentation blocks, statements, paths, calls, literals,
  interpolation, unary and binary operators, ranges, named arguments,
  resource literals, transactions, locks, try/catch/finally, and labeled
  loops.
- Reject `return`, `break`, and `continue` inside `finally`.
- Reject `==`, `&&`, `||`, unary `!`, `#`, parameter defaults,
  overloading, user-defined generics, type aliases, and alternate function
  forms.
- Build recovery so one bad line does not hide the rest of the file.
- Extract every `.mw` block from `docs/language/` as a fixture, with explicit
  tracked gaps only where the reference intentionally shows invalid code.

Done when the examples in `docs/language/` parse with source spans and the
parser has focused error-recovery tests.

## 4. Formatter

- Format the full syntax tree, not raw text fragments.
- Use canonical indentation and keyword spelling.
- Preserve documentation comments and stable IDs.
- Make formatter output parse again to the same source facts.

Done when formatter tests cover the reference sample, resource declarations,
function bodies, comments, and multiline calls.

## 5. Schema Model

- Resolve `marrow.json` with `sourceRoots`, `run.defaultEntry`,
  `store.backend`, `store.dataDir`, and `tests`.
- Match module declarations to source-root-relative paths.
- Resolve imports as module imports only.
- Build one checked-program artifact with modules, constants, functions,
  schemas, type facts, effect facts, source spans, and capability needs.
- Compile resources into typed tree schemas with identity keys, fields, child
  layers, required elements, indexes, and stable metadata IDs.
- Keep identity keys in saved paths, not stored fields.
- Reject `unknown` in managed saved fields and keys.

Done when one resource can be checked as both a local value and a saved root
through the same schema.

## 6. Saved-Tree Contract

- Define Marrow path segments with explicit kinds for roots, record keys,
  fields, child layers, indexes, and index keys.
- Define canonical saved values for primitive types and generated identity
  values.
- Implement memory storage over ordered encoded paths and bytes.
- Provide presence, exact read, write, delete, child-key listing, bounded
  subtree scan, root listing, and typed storage errors.
- Port the backend conformance style from M Rust without porting globals or M
  collation.
- Prove Marrow ordering independently of backend collation or locale.

Done when memory storage passes conformance tests for values, presence,
ordering, scans, deletes, roots, replay, limits, and errors.

## 7. Managed Writes

- Plan whole-resource writes, field writes, `delete`, and `merge` above the
  backend contract.
- Validate keys, values, required fields, sparse writes, and resource shape
  before success is visible.
- Maintain generated index trees as visible saved data.
- Reject unique conflicts without committing saved data.
- Keep managed roots protected from raw writes except in explicit maintenance
  mode.
- Keep history as ordinary keyed child layers.

Done when indexed saved resources stay coherent across successful writes,
failed writes, deletes, merges, and ordinary single-record updates.

## 8. Runtime

- Evaluate checked `.mw` functions from checked program facts.
- Implement local variables, resource values, calls, returns, control flow,
  structured errors, output, and host capabilities.
- Make `transaction` group saved writes and generated index writes.
- Treat nested transactions as savepoints.
- Make reads inside a transaction see earlier saved writes.
- Implement traversal, `exists`, `get`, `keys`, `values`, `entries`, `count`,
  `append`, and default `nextId` for single-`int` identity roots.

Done when the reference sample runs on memory storage with stable diagnostics.

## 9. Native Storage

- Add the native redb-backed store behind the same saved-tree contract.
- Use `marrow.redb`, `marrow.lock`, and a recorded format version.
- Enforce one normal writable owner per data directory.
- Support read-only inspection where possible.
- Keep redb file layout and corruption errors below the backend contract.
- Run the same conformance suite as memory storage.

Done when the reference sample runs on native storage and memory/native dumps
round-trip through the same portable path/value stream.

## 10. Tools And Release Surface

- Implement `marrow run`, `marrow test`, `marrow data`, and focused project
  commands once checked source and saved trees are real.
- Support typed inspection with source and raw inspection without source.
- Keep inspection read-only by default.
- Add backup and restore as portable ordered path/value archives.
- Add basic language services from checked source facts.
- Keep `marrow serve` optional and small: checked evaluation, exact reads,
  child lists, roots, bounded walks, and opted-in local inspection.

Done when a new user can install Marrow, run the sample, inspect saved data,
edit with basic language services, back up, restore, and understand where
persistence lives.

## Deferrals

- Do not bundle external database adapters in the first release.
- Do not add alternate language modes.
- Do not add a second storage query language.
- Do not add an ORM layer or automatic migration engine.
- Do not add a migration DSL before ordinary functions in maintenance mode
  prove insufficient.
- Do not add a hidden migration ledger to the database kernel.
- Do not add custom identity allocation policies before single-`int`
  allocation is implemented and tested.
- Do not add unchecked dynamic `any`; use `unknown` at dynamic boundaries.
- Do not add HTTP framework contracts before the language/database kernel is
  stable.
- Do not add a built-in users, roles, and permissions system before the local
  language/database kernel is real.
- Do not add external package registry work before source roots and modules
  are stable.

## Verification

Documentation-only work uses link scans, stale-term scans, and
`git diff --check`.

Parser, checker, runtime, CLI, LSP, and backend work starts with focused tests
for the changed surface, then grows to `cargo fmt --all`,
`cargo test --workspace`, and conformance suites for integration batches.
