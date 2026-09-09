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
| Durable places | Keyed store roots with one or several key components, and several roots per project. Whole-entry creation and replacement, so a present entry is complete; field and group writes through a `place` or pin that a presence proof covers (`check.requires_presence` otherwise), with proofs ended by an erase of the family or a call that erases it; required field and group-leaf reads through a proved place have their declared types; sparse and untested reads are optional; `delete` as the one clearing form; `exists`; entry identity `Id(^root)`; and each export's access demand from `marrow check`. | [Durable places](language/durable-places.md) |
| Transactions | One `transaction` block per mutating export. Every `return` inside it commits; a fault rolls the block back. | [Errors and transactions](language/errors-and-transactions.md) |
| Traversal and indexes | `for ... at most N { } on more { }` over a root, a branch, or an index; root and branch key acquisition uses at most `N + 1` bounded scans, independent of child populations; up to 8 indexes per root; a `unique` index lookup yields `Id(^root)?`. | [Traversal and indexes](language/traversal-and-indexes.md) |
| Tests | `marrow test` runs every `test` block; a durable test runs against a fresh in-memory store. | [Tests](language/tests.md) |
| CLI | `init`, `fmt`, `check`, `run`, `test`, `import`, `doctor`, `image`, and `client typescript`. | [CLI](tools/cli.md) |
| Editor server | `marrow-lsp` serves diagnostics, formatting, hover, definition, completion, signature help, and document symbols over stdio. | [Language server](tools/lsp.md) |
| Store lifecycle | `marrow import` provisions a store and populates an existing one only under its active program; `marrow run --store` runs an export against it through the companion runner, which executes only the program the store admitted; an interrupted commit reopens as `known_old`, `known_new`, or `unknown`; `marrow doctor --store` performs read-only logical inspection against the active program and reports an entry-content digest, without checking physical integrity. | [Operations](operations/README.md) |
| TypeScript client | A generated strict client and a Node supervision module over a private local channel. The runner checks List/Map length and aggregate structural size before execution and normalizes unique Map argument pairs to ascending typed key order. | [TypeScript client](tools/typescript-client.md) |

The command names `data`, `evolve`, `serve`, `backup`, and `restore` are
recognized; each reports `cli.command_unsupported`.

### Applications

Two complete applications, Club Locker (equipment lending, with a desktop
shell) and EMR (a change-set tool over a synthetic corpus), live in the
separate `marrow-acceptance` repository together with their source tests,
expectations, and the journeys that run them against a built toolchain through
the public commands. This repository keeps short reference examples,
conformance fixtures, and compiler-local regressions.

Managed-index maintenance includes entry presence independently of sparse
fields. Key-only indexes follow creation and erasure, including empty entries;
unique key-subset collisions fault and roll back the transaction. Bounded
source traversal accepts a bare non-unique index when it has no field prefix.
Logical inspection reports missing or orphaned index cells; it does not repair
pre-existing inconsistencies ([indexes](language/traversal-and-indexes.md#index-declarations)).

Resource values with generic fields support group-leaf access in either
declaration order ([resources](language/resources.md#members)).

## Not yet available

- Third-party packages ([packages](future/packages.md)).
- Closures ([general-purpose language](future/general-purpose-language.md)).
- Public aggregate inputs and bound durable values containing nominal integers;
  compilation reports `check.unsupported`. Guarded bare nominal inputs and local
  composition remain supported ([nominal ints](language/types-and-values.md#aliases-and-nominal-ints)).
- Operations over a singleton root and over a group inside a branch; each declares and checks
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
- Complete subtree enumeration and removal when absent ancestors' keys are
  unknown ([deleting](language/durable-places.md#deleting)).
- Full read-only physical-checksum verification and complete image/schema/store
  validation followed by fresh admission before recovery resumes service. The
  logical audit does not establish these requirements
  ([auditing a store](operations/README.md#auditing-a-store)).
- Bare whole-entry/group reads through a proved place and automatic traversal-pin
  presence facts carried by region. Today required fields and group leaves read bare
  through an explicit proof, and a pin is proved inside its own iteration
  ([durable programming](future/durable-programming.md)).
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

Root and branch traversal acquires its frozen keys and `more` result with at
most `N + 1` bounded scans; a family presence test uses one. Child families lie
outside that scan range, including children whose ancestors are absent.
Encountered own payload without its entry marker faults
([traversal](language/traversal-and-indexes.md#bounded-durable-traversal)). These
are engine-call bounds, not latency or memory-residency guarantees. A scan page
contains at most 64 cells with a soft 1 MiB key/value byte target; an oversized
first cell is returned to make progress
([storage](implementation/storage.md#navigating-entries)).

The toolchain builds on Linux and macOS with Rust 1.89; opening a store on disk
has its own platform and layout requirements
([Running against a store](install.md#running-against-a-store)).

## Trust boundaries

- The Rust VM entry accepts a checked function selection carrying its verified
  image. Relative function ordinals outside that image are refused at selection.
  Raw Rust arguments still require caller validation against that image's types;
  the storeless entry also requires an empty durable demand
  ([execution pipeline](implementation/README.md#pipeline)).
- Filesystem permissions and the host process protect local store files.
- Kernel operations validate supplied keys and decoded traversal/index keys
  against their declared scalar kinds and supported ranges. Mismatched stored
  keys fault before entering typed VM values; these local checks complement
  complete logical inspection by `marrow doctor`.
- Commit recovery assumes that no structurally valid foreign store or prior
  snapshot is substituted while the owner lock is held. Substitution or
  rollback of a store file under the lock is not detected. `marrow doctor`
  compares a store's contents with its program and reports a digest; it does
  not authenticate the engine file, and the digest is reported, not stored.
- Checksums and structural checks detect selected corruption; they do not
  authenticate hostile storage or prove application validity.
- `marrow doctor` does not verify physical checksums. A changed scalar that
  remains valid under its declared type can pass logical inspection. The
  inspection does not repair the engine or clear inherited unclean-shutdown
  status. Its scan pages and finding list are bounded; total native cache
  residency over large stores requires separate qualification
  ([audit implementation](implementation/storage.md#auditing-a-store)).
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
| Editor completion | 212 ms for the maximum name-chain fixture, exceeding the 150 ms budget | `29555429` (2026-09-08) | Ubuntu release CI, maximum of five after one warm request. The ordinary 10 ms selector did not run after this failure. `crates/marrow-compile/tests/query_local_syntax.rs` defines both budgets and the finite fixture corpus. |
| Workspace test wall time | 96.6 s for the unit and integration battery; 12.3 s for a settled doctest battery | `294a6290` (2026-08-31) | `cargo test --workspace --locked`, unoptimized profile, Apple M5 Pro. Two whole-battery runs measured 490 s and 840 s with a stall entering doctests whose cause was not established. |
| Clean Rust build | 7.4 s | `294a6290` (2026-08-31) | Workspace build into an empty target, unoptimized profile, Apple M5 Pro. |
| Incremental Rust build | 0.26 s after touching `marrow`; 0.72 s after touching `marrow-compile` | `294a6290` (2026-08-31) | Median over warm mtime-only touches, unoptimized profile. |

The three clocks and the design rules they impose are described in
[Compilation and test speed](implementation/speed.md#three-clocks).
