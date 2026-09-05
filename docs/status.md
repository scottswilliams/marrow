# Project status

Marrow is unreleased. The tables below say what the toolchain does at this
revision and what is future work; the [language reference](language/README.md) defines
each behavior.

## What works

| Area | Today | Page |
|---|---|---|
| Language core | Modules, functions, generics, `const` and `var`, `if` and `if const`, `match`, `while`, bounded `for`, let-else, `require`, prefix `try`, checked arithmetic, and `test` blocks. Braces delimit blocks. | [Source and syntax](language/source-and-syntax.md), [Control flow](language/control-flow.md) |
| Values and types | Scalars, `date`, `instant`, `duration`, optionals `T?`, structs, enums, `Option` and `Result`, lists and maps, global name/optional-name aliases and nominal ints, generic types. Every value copies by value. | [Types and values](language/types-and-values.md) |
| Resources | Required and sparse fields, groups, keyed branches nested to 16 levels, and local resource values. | [Resources](language/resources.md) |
| Durable places | Keyed store roots with one or several key components, and several roots per project. Field and whole-entry reads and writes, `exists`, `place`, `delete`, entry identity `Id(^root)`, and each export's access demand from `marrow check`. | [Durable places](language/durable-places.md) |
| Transactions | One `transaction` block per mutating export. Every `return` inside it commits; a fault rolls the block back. | [Errors and transactions](language/errors-and-transactions.md) |
| Traversal and indexes | `for ... at most N { } on more { }` over a root, a branch, or an index; up to 8 indexes per root; a `unique` index lookup yields `Id(^root)?`. | [Traversal and indexes](language/traversal-and-indexes.md) |
| Tests | `marrow test` runs every `test` block; a durable test runs against a fresh in-memory store. | [Tests](language/tests.md) |
| CLI | `init`, `fmt`, `check`, `run`, `test`, `import`, `image`, and `client typescript`. | [CLI](tools/cli.md) |
| Editor server | `marrow-lsp` serves diagnostics, formatting, hover, definition, completion, signature help, and document symbols over stdio. | [Language server](tools/lsp.md) |
| Store lifecycle | `marrow import` provisions a store; `marrow run --store` runs an export against it through the companion runner; an interrupted commit reopens as `known_old`, `known_new`, or `unknown`. | [Operations](operations/README.md) |
| TypeScript client | A generated strict client and a Node supervision module over a private local channel. | [TypeScript client](tools/typescript-client.md) |

The command names `data`, `doctor`, `evolve`, `serve`, `backup`, and `restore`
are recognized; each reports `cli.command_unsupported`.

### Applications

Two complete applications, Club Locker (equipment lending, with a desktop
shell) and EMR (a change-set tool over a synthetic corpus), live in the
separate `marrow-acceptance` repository together with their source tests,
expectations, and the journeys that run them against a built toolchain through
the public commands. This repository keeps short reference examples,
conformance fixtures, and compiler-local regressions.

## Not yet available

- Third-party packages ([packages](future/packages.md)).
- Closures ([general-purpose language](future/general-purpose-language.md)).
- Operations over a singleton root, over a root whose resource declares a
  nominal field, and over a group inside a branch; each declares and checks
  today ([durable places](language/durable-places.md)). A group inside another
  group is `check.unsupported` at its declaration
  ([resources](language/resources.md)).
- A keyed scalar leaf such as `tags[pos: int]: string`; a branch holds scalar
  fields ([resources](language/resources.md)).
- `decimal` ([types and values](language/types-and-values.md)).
- Index rename and retirement ([traversal and indexes](language/traversal-and-indexes.md)).
- Schema evolution. Today a changed durable contract is a
  `store.contract_changed` refusal and the prior program stays usable
  ([admission and activation](future/admission-and-activation.md)).
- Backup and restore ([local applications](future/local-applications.md)).
- Checked durable place bindings, stable required-field reads through a
  presence-tested address, and complete-entry-only creation. The selected first
  increment keeps serial execution and requires a source/new-store migration
  ([durable programming](future/durable-programming.md)). Current `place`, field
  creation and commit-time required-missing behavior remain implemented until
  that vertical change lands.
- Local reader/writer overlap and served execution with several terminals and
  public paths. The selected one-store model keeps mutating invocations serial
  ([served execution](future/served-execution.md)).
- Path authority: principals and grants finer than read and write
  ([path effects and authority](future/path-effects-and-authority.md)).
- Signed releases and a release promise ([compatibility](compatibility.md)).

## Bounds and platform

Every limit is a fixed number: source nesting, declaration counts, key
components, indexes per root, member and value depth, the instruction budget,
and text and collection sizes ([Execution limits](language/execution-limits.md#limits)).

One current limitation lies outside those bounds: durable traversal and family
presence tests skip descendant-only entries one at a time. The number skipped
is not bounded by `at most N` or by the invocation's instruction budget
([traversal](language/traversal-and-indexes.md#bounded-durable-traversal)). Bounded
family navigation is part of the selected
[durable-language direction](future/durable-programming.md#evidence); it is not
implemented yet.

The toolchain builds on Linux and macOS with Rust 1.89; opening a store on disk
has its own platform and layout requirements
([Running against a store](install.md#running-against-a-store)).

## Trust boundaries

- Filesystem permissions and the host process protect local store files.
- Commit recovery assumes that no structurally valid foreign store or prior
  snapshot is substituted while the owner lock is held. Substitution or
  rollback of a store file under the lock is not detected.
- Checksums and structural checks detect selected corruption; they do not
  authenticate hostile storage or prove application validity.
- Encryption at rest is delegated to the filesystem or substrate.
- TLS, authentication, identity providers, operator credentials, and hardware
  durability are deployment responsibilities.
- Static checking cannot establish application intent, correct policy design,
  regulatory compliance, or absence of external side channels.

The supply chain has a floor:

- The workspace carries no `unsafe` code; CI runs `cargo clippy --workspace
  --all-targets -- -D warnings -F unsafe-code`, which fails on any.
- An advisory CI job runs `cargo audit` over the committed `Cargo.lock` and
  emits a CycloneDX bill of materials. An advisory is triaged as a finding and
  does not block integration.
- A new dependency requires maintainer approval and a license review
  ([contributing](../CONTRIBUTING.md)).
- Tamper evidence, an audit trail, encryption at rest, and image authenticity
  are future work ([served execution](future/served-execution.md)).

## Measurements

Each figure names the revision and method it was taken with. A figure taken at
one revision is not restated as current at another, and no figure transfers to
another machine.

| Clock | Figure | Revision | Method |
|---|---|---|---|
| Compile time of a `.mw` program | 12.5 ms median for `marrow check` over a 2,278-line, 2,000-field program; slowest of 31 runs 13.1 ms | `294a6290` (2026-08-31) | Release binary, one fresh process per run, warm filesystem, Apple M5 Pro. The program is `crates/marrow/tests/fixtures/v01/e07_m_corpus/clinical`; the timing harness is not in the repository. |
| Editor completion | Under 1 ms for a 64 KiB file; 54 ms for a maximum admitted file in its densest shape | Not recorded | Worst of five after a warm request, three runs, optimized profile. The figures are recorded with the budgets in `crates/marrow-compile/tests/query_local_syntax.rs`; the release CI leg asserts the 10 ms and 150 ms budgets. |
| Workspace test wall time | 96.6 s for the unit and integration battery; 12.3 s for a settled doctest battery | `294a6290` (2026-08-31) | `cargo test --workspace --locked`, unoptimized profile, Apple M5 Pro. Two whole-battery runs measured 490 s and 840 s with a stall entering doctests whose cause was not established. |
| Clean Rust build | 7.4 s | `294a6290` (2026-08-31) | Workspace build into an empty target, unoptimized profile, Apple M5 Pro. |
| Incremental Rust build | 0.26 s after touching `marrow`; 0.72 s after touching `marrow-compile` | `294a6290` (2026-08-31) | Median over warm mtime-only touches, unoptimized profile. |

The three clocks and the design rules they impose are described in
[Compilation and test speed](implementation/speed.md#three-clocks).
