# Marrow Roadmap

This roadmap tracks the work needed to make the implementation match the
language reference in [`../language/`](../language/) and the runtime design in
[`../implementation.md`](../implementation.md).

Marrow is unreleased. Keep this page focused on the next product shape. Old
designs become code, tests, concise reference docs, or deletions.

## Release Shape

The first real Marrow release presents:

- the `marrow` command;
- documented `.mw` syntax, formatting, checking, and runtime behavior;
- resources as typed trees for local and saved data;
- a small native project store;
- visible generated indexes;
- stable errors and structured CLI output;
- typed and raw inspection tools;
- portable backup and restore;
- a clear backend contract proven by memory and native storage.

The release does not require a background service, a remote database, a broad
standard library, a web framework, or backend-specific application APIs.

## Guardrails

1. `docs/language/` is the source of truth for `.mw`.
2. `docs/implementation.md` is the source of truth for runtime, backend,
   server, and inspection architecture.
3. Saved data stays behind the ordered-tree backend contract.
4. Native storage is the default local project store.
5. Database-specific adapters stay outside the default repository and release.
6. Stale design notes are deleted or folded into reference docs.

## Build Order

Build from the smallest language/database kernel outward.

| Step | Surface | Proves |
|---|---|---|
| 1 | Reference spine | There is one clear product target. |
| 2 | Source pipeline | Documented `.mw` examples parse and format. |
| 3 | Schema model | Local and saved resources share one tree shape. |
| 4 | Managed storage | Saved writes, indexes, and failures stay coherent. |
| 5 | Consistency | Transactions, locks, traversal, and IDs have simple rules. |
| 6 | Portability | Stores and archives share one ordered-tree contract. |
| 7 | Tools | Users can run, inspect, edit, back up, and restore. |
| 8 | Release | The sample runs on native storage with stable diagnostics. |

Each slice leaves a useful surface behind. Prefer one complete path over
several partial paths.

## 1. Reference Spine

Lock the product model before expanding implementation work.

- Keep language behavior in `docs/language/`.
- Keep runtime, backend, server, and inspection design in
  `docs/implementation.md`.
- Keep install, error, README, and roadmap pages pointed at those references.
- Maintain the compact sample in [`../language/sample.md`](../language/sample.md).
- Keep the source, runtime, storage, and tooling docs free of obsolete
  implementation assumptions.
- Delete bundled database-specific adapters or move them out of the default
  workspace.

Done when the language docs, implementation reference, and sample describe the
same product: typed resources, saved trees, managed writes, visible indexes,
explicit persistence, and native local storage.

## 2. Source Pipeline

Make source text match the reference.

- Parse indentation-delimited `.mw` blocks.
- Parse modules, imports, resources, fields, keyed layers, stable `@id(...)`
  metadata, indexes, and history-layer patterns.
- Map module names to source-root-relative paths and reject mismatches.
- Keep imports as module imports only; reject wildcard, renamed, and path
  imports.
- Parse module-level `const`, local `let` and `var`, `out`, `inout`, named
  arguments, resource literals, direct tree iteration, and labeled loops.
- Parse `transaction`, `lock`, `try`, `catch`, `finally`, `throw`, and
  conversion calls.
- Reject `return`, `break`, and `continue` inside `finally`.
- Check `throw` values, `catch` bindings, `finally` cleanup rules, and typed
  error propagation.
- Use the documented type names only: `int`, `decimal`, `bool`, `string`,
  `bytes`, `date`, `instant`, `duration`, `ErrorCode`, and `unknown`.
- Use `instant` for UTC points in time. Do not add a source-level `time`
  type in the first release.
- Treat a missing function return type as "no returned value"; do not add an
  explicit source-level `void` type.
- Treat assignment as statement-only.
- Preserve left-to-right evaluation for operands and call arguments.
- Require explicit scalar conversions.
- Report numeric overflow, invalid conversion, and unrepresentable arithmetic
  as typed numeric errors.
- Restrict ranges to `int` endpoints.
- Accept `=` as equality in expressions and conditions.
- Reject `==`, `&&`, `||`, unary `!`, and `#`.
- Reject parameter defaults and function overloading.
- Require explicit `fn` declarations. Reject `proc`; `.mw` uses one function
  form for effectful and value-returning code.
- Keep function visibility to `pub` or module-private; reject `internal` and
  explicit `private`.
- Reject user-defined generic functions and generic type declarations.
- Reject user-defined type aliases.
- Resolve `std::` imports through concrete signatures and host capabilities.
- Format full keyword spellings and canonical indentation.
- Build the `.mw` parser, formatter, checker, and editor model as a native
  source pipeline.
- Replace brace-era `.mw` fixtures and embedded renderer sources with the
  reference syntax.
- Retire obsolete `.mw` operators such as `==`, `&&`, and `||` once parser
  fixtures use `=`, `and`, and `or`.
- Prefer `throw Error(...)` in parser fixtures.

Done when the examples in `docs/language/` parse and format without fixture
syntax.

## 3. Schema Model

Build one resource schema model for checking, runtime, tools, and saved data.

- Resolve `marrow.json` with source roots, entry defaults, store selection,
  data directory, and tests.
- Keep credentials, compiled schemas, migration history, permissions, and
  backend app APIs out of `marrow.json`.
- Treat module-less `.mw` files as scripts or entrypoints, not importable
  modules.
- Keep module-level functions, constants, resources, and imported short module
  names in one namespace.
- Keep resources as schema declarations without resource visibility markers.
- Keep top-level constants module-private and compile-time.
- Support local resources, keyed saved roots, and singleton saved roots.
- Store saved resources as typed tree fields and layers, not hidden blobs.
- Check leaf fields, keyed layers, unkeyed groups, required elements, sparse
  reads, resource literals, identity types, `exists(...)`, `get(...)`, `out`,
  and `inout`.
- Treat composite resource identities as one generated identity type.
- Address managed roots with generated identity values, not raw key tuples.
- Reject `unknown` in managed saved fields and keys.
- Keep raw saved-tree access at import, export, migration, repair, and tooling
  boundaries.
- Lower logical saved paths into canonical encoded segments with segment kinds
  that prevent collisions.
- Produce one checked-program artifact with modules, schemas, type and effect
  facts, source spans, and capability needs.
- Use checked-program facts for runtime, CLI diagnostics, LSP features,
  inspection, generated docs, and migration planning.
- Keep the store below that layer; it receives encoded paths and bytes, not
  source facts.
- Feed schema metadata to docs, hover, completion, inspection, rename, and
  migration tooling.

Done when a resource can be created locally, saved, read back, inspected, and
checked through the same schema.

## 4. Managed Storage

Make saved writes coherent above the backend contract.

- Implement one managed write planner for whole-resource writes, field writes,
  `delete`, and `merge`.
- Validate keys, values, required fields, sparse writes, and resource shape
  before exposing a successful write.
- Maintain generated indexes as visible saved trees.
- Limit declared indexes to direct members of keyed saved resources.
- Make non-unique indexes enumerable by identity and unique indexes point to
  one identity.
- Require non-unique indexes to end with all identity keys in declaration
  order.
- Reject index arguments that walk through keyed child layers.
- Populate index entries only when every indexed value is present.
- Reject unique conflicts without committing saved data.
- In shared-writer profiles, require a capability that can safely reserve a
  unique index entry.
- Keep generated index writes inside the managed write or transaction.
- Keep history as explicit keyed child layers; do not add a special history
  keyword or automatic audit writes.
- Protect managed roots from raw writes except in maintenance mode.
- Treat cascade cleanup as application or migration code.
- Report failed rollback or restoration as a storage failure that needs
  inspection or repair.

Done when indexed saved resources stay coherent across successful writes,
failed writes, deletes, merges, and ordinary single-record updates.

## 5. Consistency And Traversal

Define the behavior users feel while programs run.

- Make a single managed write internally coherent.
- Make `transaction` group several saved writes and generated index writes.
- Treat nested transactions as savepoints.
- Commit transactions that leave by `return`, `break`, or `continue`.
- Roll back a transaction only when an error escapes.
- Keep local variables, output, and host effects outside saved-data rollback.
- Make reads inside a transaction see earlier saved writes from that
  transaction.
- Use locks for application invariants, not schema validation or security.
- Release locks on every block exit path.
- Implement typed `for` over resources, keyed layers, saved trees, and index
  branches.
- Implement `keys`, `values`, `entries`, `count`, `append`, and `nextId`.
- Keep low-level ordered stepping in backend and tooling APIs until `.mw` has
  a clear cursor or optional-value model.
- Reject or report writes to the same layer being traversed.
- Make `append` choose the next key after the highest populated positive
  integer key, without filling holes or renumbering.
- Provide default `nextId` allocation for single-`int` identity roots.
- Reject `nextId` for composite or non-integer identity roots in ordinary
  `.mw`.
- After restore, rebuild allocator state so `nextId` does not reuse an
  existing identity.

Done when traversal examples run against local and saved trees with the same
visible behavior, and transaction, lock, append, and ID edge cases have focused
tests.

## 6. Portability And Backends

Keep storage replaceable without changing `.mw`.

- Keep memory and native storage behind the same backend contract.
- Use saved-tree product names for public backend APIs.
- Do not include database-specific adapter crates in the default workspace.
- Prove Marrow key order independently of backend collation or locale.
- Test backend presence, exact values, child ordering, subtree scans, bounded
  deletes, roots, dumps, replay, and typed storage errors.
- Keep native storage local-first with one normal writable owner per data
  directory.
- Use `marrow serve` for shared local tooling sessions when direct store open
  is not enough.
- Define the portable archive as an ordered canonical path/value stream plus a
  small manifest.
- Preserve segment kinds in archives, diffs, and restore.
- Include generated index trees in normal data backups.
- Verify or rebuild generated indexes during typed restore when source is
  available.
- Keep whole-root managed deletes and non-empty restore targets in explicit
  maintenance modes.
- Make normal restore target an empty store; keep replace, merge, and repair
  restore modes explicit.
- Run migrations by calling named `fn` declarations in explicit maintenance
  mode.
- Cover sparse-field additions, required-field additions, index backfills, and
  saved-data renames as separate migration cases.
- Check capabilities by promise, not backend name.
- Treat each external engine as an out-of-tree adapter, not a language target.

Done when the reference sample runs on memory and native storage through the
common contract, and no bundled external database adapter is required.

## 7. Tools And Release Surface

Make the product usable without turning tools into a second database.

- Align CLI diagnostics, JSON/JSONL output, and error envelopes.
- Promote dotted Marrow error codes across CLI and server output.
- Implement the first standard-library modules with concrete signatures:
  UTC `instant` clock helpers, text, bytes, math, IO, env, assert, and log.
- Use documentation comments and stable IDs for hover, completion, rename,
  generated docs, and inspect output.
- Support typed inspection with source and raw inspection without source.
- Keep inspection read-only by default.
- Use `marrow data` for raw saved-data tooling.
- Use `--root` for journal saved-root filters.
- Make maintenance mode explicit and require changed-root reports.
- Keep `marrow serve` as a small tooling protocol: checked evaluation, exact
  reads, child lists, roots, bounded walks, and opted-in local inspection.
- Expose saved-path protocol request names for the Marrow product surface.
- Keep protocol writes behind checked Marrow execution or explicit repair and
  migration commands.
- Keep local IPC as the default transport.
- Require explicit operator choice, authentication, and transport security for
  non-loopback TCP.
- Document project data selection, inspection, backup, and restore.
- Package the release without stale names or obsolete design notes.

Done when a new user can install Marrow, run the sample, inspect saved data,
edit with basic language services, back up, restore, and understand where
persistence lives.

## Deferrals

- Do not expand the standard library before resources, saved data, and
  traversal are coherent.
- Do not use standard-library overloads or hidden dynamic dispatch to avoid
  precise type checking.
- Do not add target-typed JSON conversion helpers before resource encoding and
  conversion rules are stable.
- Do not bundle external database adapters in the first release.
- Do not make the server protocol a general remote database API.
- Do not add a separate storage query language.
- Keep keyed-resource iteration source-level: `for id in ^books` before any
  lower-level cursor surface.
- Do not add an ORM layer or automatic migration engine.
- Do not add a migration DSL or `migration` keyword before ordinary functions
  in maintenance mode prove insufficient.
- Do not add a hidden migration ledger to the database kernel.
- Do not add custom identity allocation policies before single-`int`
  allocation is implemented and tested.
- Do not add an unchecked dynamic `any` type; use `unknown` at dynamic
  boundaries.
- Do not add a time-of-day or timezone-aware calendar type before `date`,
  `instant`, and `duration` are implemented and tested.
- Do not add implicit async or in-language threads.
- Do not standardize HTTP framework contracts before the language/database
  kernel is stable.
- Do not add a built-in users, roles, and permissions system before the local
  language/database kernel is real.
- Do not add external package registry work before source roots and modules
  are stable.
- Do not include alternate language modes in the default `.mw` learning path.

## Verification

Documentation-only work uses link scans, stale-term scans, and
`git diff --check`.

Parser, checker, runtime, CLI, LSP, and backend work starts with focused tests
for the changed surface. Run workspace build and test gates for broad
integration batches.
