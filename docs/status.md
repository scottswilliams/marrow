# Project status

Marrow is unreleased. The tables below say what the toolchain does at this
revision and what is future work; the [language reference](language/README.md) defines
each behavior.

[Beta scope](vision.md#beta-scope) selects useful storeless programs and a
recoverable local application. It is a target, not the state of this revision.
Native opening, recovery and fresh restore retain the accepted Head's physical
addresses. This does not implement additive contract activation.

Ordinary enum composition, local source reuse, additive updates and full
application-lifetime qualification still require work. Broader future features are not
prerequisites.

## What works

| Area | Today | Page |
|---|---|---|
| Language core | Modules, functions, generics, `const` and `var`, `if` and `if const`, `match`, `while`, bounded `for`, let-else, `require`, prefix `try`, checked arithmetic, and `test` blocks. Braces delimit blocks. | [Source and syntax](language/source-and-syntax.md), [Control flow](language/control-flow.md) |
| Values and types | Scalars, `date`, `instant`, `duration`, optionals `T?`, structs, enums, `Option` and `Result`, lists and maps, global name/optional-name aliases and nominal ints, generic types. Every value copies by value. | [Types and values](language/types-and-values.md) |
| Resources | Required and sparse fields, groups, keyed branches nested to 16 levels, and local resource values. | [Resources](language/resources.md) |
| Durable places | Keyed store roots with one or several key components, and several roots per project. Whole-entry creation and replacement, so a present entry is complete; field and group writes through a `place` or pin that a presence proof covers (`check.requires_presence` otherwise), with proofs ended by an erase of the family or a call that erases it; required field and group-leaf reads through a proved place have their declared types; sparse and untested reads are optional; `delete` as the one clearing form; `exists`; entry identity `Id(^root)`; and each export's access demand from `marrow check`. | [Durable places](language/durable-places.md) |
| Transactions | One `transaction` block per mutating export. Every normal function exit inside it commits, including `try` and `require` failure; a fault before commit rolls the block back. | [Errors and transactions](language/errors-and-transactions.md) |
| Traversal and indexes | `for ... at most N { } on more { }` over a root, a branch, or an index; root and branch key acquisition uses at most `N + 1` bounded scans, independent of child populations; up to 8 indexes per root; a `unique` index lookup yields `Id(^root)?`. | [Traversal and indexes](language/traversal-and-indexes.md) |
| Tests | `marrow test` runs `test` blocks through ordinary function calls. Each durable test has a fresh in-memory store; transaction-owning calls commit setup, and private readers can observe it. Direct durable operations and calls to mutating non-owner helpers are refused in test bodies. | [Tests](language/tests.md) |
| CLI | `init`, `fmt`, `check`, `run`, `test`, `import`, `doctor`, `recover`, `backup`, `restore`, `image`, and `client typescript`. `run --stdin` supplies one bounded UTF-8 string argument; bare-string results are bounded, JSON data construction checks its byte limit before appending, and rendering or result-delivery failures fail the command. | [CLI](tools/cli.md) |
| Editor server | `marrow-lsp` serves diagnostics, formatting, hover, definition, completion, signature help, and document symbols over stdio. Whole-analysis resource stops complete the affected revision with request refusals, an unlocated explanation, and retractions of prior diagnostics. A later edit that permits project capture and analysis can recover. | [Language server](tools/lsp.md) |
| Store lifecycle | `marrow import` provisions or populates a store under its active program; `marrow run --store` runs through the admitted companion. Pending activation blocks ordinary access. `marrow recover --store` validates the exact stored image, physical integrity and logical contents, then establishes fresh activation barriers without replaying a missing head update. `marrow doctor --store` remains read-only logical inspection without physical verification. Doctor and backup preserve source artifacts, including ownership-marker bytes and absence. Logical backup carries the exact image, head and complete entry/index families; restore validates a fresh store without compiling current source. | [Operations](operations/README.md) |
| TypeScript client | A generated strict client and a Node supervision module over a private local channel. The runner checks List/Map length and aggregate structural size before execution, normalizes unique Map argument pairs to ascending typed key order, and bounds outbound frame construction before appending. Provision records retain publication/activation uncertainty or primary failure plus failed cleanup. Authenticated native startup distinguishes activation uncertainty from invocation outcomes. Missing delivery remains uncertain. | [TypeScript client](tools/typescript-client.md) |

The command names `data`, `evolve`, and `serve` are
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

Branch-field annotations are resolved once per admitted resource Product. Roots
sharing that Product reuse its canonical field scalars when building executable
branch descriptors ([compiler pipeline](implementation/README.md#pipeline)).

Refused generic definitions retain their names for duplicate checking
([generic types](language/types-and-values.md#generic-types)). Enums exceeding
the member limit also retain their names and original refusal
([enums](language/types-and-values.md#enums)).

A refused function body does not suppress recursion or transaction diagnostics
in an independent complete call component. Generic bodies continue draining after
an ordinary body refusal. Effects that require a complete call chain exclude
missing or cyclic bodies and their callers; an unfilled function slot cannot encode
([compiler pipeline](implementation/README.md#pipeline)).

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
- Complete subtree enumeration and removal when absent ancestors' keys are
  unknown ([deleting](language/durable-places.md#deleting)).
- Read-only physical-checksum verification by `marrow doctor`. Explicit recovery
  uses a read-write physical integrity check and exact-image logical validation;
  a logical audit alone does not establish those properties
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
- Editor completion inside its latency budget for the maximum name-chain
  fixture; the budget in `crates/marrow-compile/tests/query_local_syntax.rs`
  records the overrun.
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

- The runner bounds image input before verification to the 512 KiB image limit
  plus one excess byte. Oversized images are refused with `image.envelope`
  ([execution limits](language/execution-limits.md#limits)).
- Verification bounds what each pass retains per instruction, not what a whole
  image costs: there is no total verifier memory or work budget
  ([execution pipeline](implementation/README.md#pipeline)).
- The verifier and the store admission fence accept only the supported image and
  logical-head generations, and refuse anything else before the engine opens.
  Older artifacts and data require their matching tools
  ([compatibility](compatibility.md#versioning)).
- The Rust VM entry accepts a checked function selection carrying its verified
  image. Relative function ordinals outside that image are refused at selection.
  Raw Rust arguments still require caller validation against that image's types;
  the storeless entry also requires an empty durable demand
  ([execution pipeline](implementation/README.md#pipeline)).
- VM string conversion checks each contribution before appending it, and faults
  with `run.text_limit` past its byte limit. This bounds the constructed text's
  length, not allocator capacity or total VM memory
  ([execution limits](language/execution-limits.md#limits)).
- Filesystem permissions and the host process protect local store files.
- A code-only rebind assumes cooperating access to the store directory: external
  writes to the same inode are not detected
  ([native owner](implementation/storage.md#native-owner)).
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
- `marrow doctor` does not verify physical checksums, repair the engine, or
  clear inherited unclean-shutdown status, and its native cache residency over
  large stores is unqualified
  ([auditing a store](operations/README.md#auditing-a-store)).
- Encryption at rest is delegated to the filesystem or substrate.
- TLS, authentication, identity providers, operator credentials, and hardware
  durability are deployment responsibilities.
- Static checking cannot establish application intent, correct policy design,
  regulatory compliance, or absence of external side channels.

The supply chain has a floor:

- The workspace carries no `unsafe` code; every CI run denies it with
  `clippy -F unsafe-code` ([checks](../CONTRIBUTING.md#checks)).
- A weekly advisory workflow runs `cargo audit` over the committed `Cargo.lock`
  and emits a CycloneDX bill of materials. An advisory is triaged as a finding
  and does not block integration.
- A new dependency requires maintainer approval and a license review
  ([contributing](../CONTRIBUTING.md)).
- Tamper evidence, an audit trail, encryption at rest, and image authenticity
  are future work ([served execution](future/served-execution.md)).

## Measurements

No figure is recorded here. A measurement belongs to the revision, workload,
platform and method it was taken with, is not restated as current at another
revision, and does not transfer to another machine. [Compilation and test
speed](implementation/speed.md#three-clocks) describes the three clocks, the
design rules they impose, and how a broad gate records them.
