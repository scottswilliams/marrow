# Marrow v0.1 Roadmap

The goal: total production-foundation readiness for the v0.1 prototype — a complete cleanup and
hardening of the entire project (language, database, tooling, tests, docs), executed as waved
lanes ending at the v0.1 tag. The plan synthesizes three adversarially verified whole-system
reviews (architecture, subtraction, innovation); their long-form evidence lives outside the
repository, and this file is the executable authority. Proposal tags (`II.A2`, removal titles,
`C26`, `G1-4`) are provenance only — every lane states its own scope. Two sibling repositories
are load-bearing: `marrow-decisions`, checked out beside this repository, holds the decision
records cited as `foundations/NN`, `language/NN`, `model-ir/NN`, `storage-engine/NN`,
`evolution/NN`, `catalog-identity/NN`, and `tooling/NN`; `marrow-lsp` is the sibling editor
tooling that consumes the analysis API.

## Orchestration

This file is the single forward-only backlog for an orchestration loop:

1. **Gates are decided.** Wave 0 records the binding `Decided:` outcome for every gate; an
   executing lane implements the decided text without re-litigating. The evidence-protocol gates
   (35; 48's drafting step) execute per their recorded protocols. If execution uncovers evidence
   that a decision is unsound, stop the lane and bring the specific contradiction to the owner —
   never silently deviate, never re-open silently. Language-surface decisions are settled in
   Wave 0; lanes do not propose new surface.
2. **Lane selection.** Work the earliest wave with open lanes. `∥` lanes fan out concurrently —
   each in an isolated worktree with its own external `CARGO_TARGET_DIR`, spelled explicitly in
   every cargo command; `→` lanes run in stated order. Store-crate and checker lanes are known
   hotspots: they fan out only when file-disjoint within the crate; same-file concurrency never.
3. **The lane loop.** Per lane: TDD seed first (watch it fail); the build agent makes it pass
   and owns the comprehensive cleanup of its area; two-lens adversarial review (a soundness
   lens that probes with real repros, an idiom/spec lens that inspects the touched Rust); every
   finding fixed; re-review until genuinely clean. Full gate at integration: build, full tests,
   `clippy -D warnings` at zero, fmt clean, zero `unsafe`, no new deps, fresh output only.
   Re-check main's HEAD and rebase immediately before integrating; never push over in-progress
   work.
4. **Forward-only status.** Delete a lane when its evidence packet is complete and integrated.
   Annotate a blocked lane with its blocker. Never append history, completed-work notes, or
   migration narration to this file.

Operating principles for this plan:
- **Subtraction-window rule.** Language-surface and byte-format cuts have no "later": post-release
  they are breaking changes. Every accepted cut executes in v0.1 regardless of its review's P2/P3
  label; a declined cut is a keep-forever decision, not a deferral. This is why several removals
  the subtraction review rated P2/P3 appear in implementation waves below.
- **One lane owns each file.** Where REPORT entries and removals touch the same mechanism, the
  fold is stated in Wave 0 and a single lane carries both.
- **Removals are work items** with the same seed/review/evidence rigor as additions, including an
  absence/sibling scan across the lane's whole area.
- **Live co-author.** The engine-resident-catalog refactor is in flight on main. All Wave 3
  catalog/evolution lanes coordinate with (or are executed by) that effort; rebase before
  integrating, never push over it.
- **Conformance-corpus campaign.** A coordinated test-architecture effort owns
  fixtures/v01/check/ and fixtures/v01/run/ (new, additive subtrees), the two corpus driver
  suites (the marrow-check fixture walker; the `marrow test` corpus runner in crates/marrow
  tests), and amendments to docs/testing-architecture.md. Roadmap lanes do not edit those files
  or create competing test-architecture authorities; once the checker driver lands, lanes write
  new Tier-1 checker tests as corpus fixtures rather than new Rust call sites. The campaign
  never touches cmd_test.rs, discovery.rs, or the assert stdlib (W6.2/W6.18 own them); its
  runtime driver sequences after W6.2 and consumes that record contract. Bulk checker-suite
  migration runs only after Wave 5; remaining migration and the binary fold execute as W7.1
  scope. Ledger-pinned suites in docs/freeze-gate-evidence.md keep their verification commands
  until the owning lane updates the ledger rows in the same commit.

Lane entry format — Carries: proposal ids / removal titles. Owns: crates/files. Deletes: removal
targets. Seed: the first failing check. Review: adversarial focus. Done: evidence beyond green.
`∥` = may run concurrently within its wave (file-disjoint); `→` = must sequence. Lanes execute
in listing order within a wave; lane ids are names, not order. Single-row additions to
docs/error-codes.md are mechanical-rebase edits and never block `∥` marking; section-level
rewrites of a shared page require explicit ordering. References to CI mean the repo's recorded
gate script set — the full integration bar run fresh at every lane integration and wave gate; a
check added to CI joins that recorded set (no hosted CI exists in v0.1).

---

## Wave 0 — Decisions (all 52 decided 2026-06-10)

Every propose-first item, every Accepted-ADR revisit, every cross-review conflict, and every
innovation seam — 52 numbered gates in eight groups (A-G plus C′), retained for navigation.
Every gate is decided and binding; an executing lane implements the decided text. The two
evidence-protocol gates (35; 48's drafting step) execute automatically per their recorded
protocols with no further sitting.

### Group A — Record identity, presence, and data validity

**1. Presence totality (II.A2 a, c, d).** Decided: (a) yes — iteration-derived identities
(for id in ^books / ^books.byIndex(...)) bind id with a record-presence proof scoped to the
loop body, sharing the existing narrowing-invalidation algebra, sound under the W2.5
single-writer contract. (c) yes — unproven identities (parameters, identities read from
Id(^store) fields, gate-2 constructed identities) require resolution at the first record read
via check.bare_maybe_present_read; the over-broad Declaration-source discharge through arbitrary
identities is deleted; the early-return guard form `if not exists(place)` with a diverging body
installs narrowing for the remainder of the enclosing scope (no `require <cond> else` form is
pulled forward in v0.1); validation protocol: W2.7 runs the tightened checker over the full
fixture and docs corpora before freezing, and any legitimate pattern that cannot be resolved by
the supported forms (??/else-binding/if exists/optional chaining/early-return guard/gate-38
binding guard) is fixed by adding a narrowing-source rule or migrating the fixture to the v0.1
contract — never by re-adding Declaration discharge; (c) itself is unconditional. (d) fork (i) —
ADRs language/02 and model-ir/02 stand: once (a)+(c) make record-presence checking total,
dynamic absence at a proven saved read is a fatal data-attachment fault — the runtime's
catchable run.absent_element path for saved reads is removed (not catchable, not reachable by
??), the spec states that only proven reads can fault and only as corruption/data-attachment
failure, the III.A2 absent-record-vs-missing-cell distinction is the normative wording,
error-codes.md:229 is rewritten to match, and the spec never states that ?? resolves
run.absent_element (?? is compile-time resolution of maybe-presence).

**2. Identity constructor — build vs honest docs (II.A3 vs removal #1).** Decided: build the
constructor in v0.1. Spelling `Id(^store, key...)` — the type spelling reused in expression
position; static checks are key arity and scalar types against the store's declared keyspace
(check.key_type on mismatch) with `unknown` arguments rejected (convert first); the runtime
reuses the existing identity-splice guards (root provenance, arity, per-key scalar kind); the
result carries no presence proof (composes with gate 1c). The fictional
loadBookId/loadEnrollmentId helpers are deleted in the same change with the three broken spec
pages rewritten to the real constructor — removal #1's core deletion is carried, its
compensating honest-absence notes and docs/future constructor entry are superseded and not
written; `Id` reserved-word bookkeeping rides II.E2 (W4.6); the docs-only fallback clause is
void.

**3. Multi-store retire accounting (IV.D3).** Decided: Option A — discharge counts retire work
across all owning roots, sharing apply's place-enumeration helper so check and run have one
owner and cannot drift; the resulting witness-content change (counts summed across roots) is
reviewed as a deliberate semantic contract change with the frozen witness-equality tests updated
under that review. The IV.D3 keying half lands in the same lane: source_kinds lookups keyed by
(kind, path), and apply_retires gains the fail-closed at-most-one-active-entry-per-path
ambiguity assertion. Option B (reject multi-store resources) is rejected.

**4. Identity-typed index components + index-argument typing rule (II.C1).** Decided: accept.
(1) Id(^store)-typed top-level fields are valid index components; lookup arguments are nominally
checked; for-loops over such branches stream the stored resource's identities; component
encoding is the existing order-preserving identity payload with store-ID prefix per
storage-engine/04. (2) Enum index arguments are explicitly accepted, ordered by stable member
identity per storage-engine/03's existing normative ordering law. (3) storage-engine/03's
index-component sentence is amended to license 'scalar, enum, or identity-typed fields' (naming
enums, since this gate licenses them), and language/04's identity-key deferral is scoped: store
and keyed-layer keys remain deferred, index components are licensed — types.md:105 scoped to
match; the full index-argument typing rule (including enums) is written into
resources-and-storage.md. (4) Fold resolved: the removal 'Delete the unreachable
StoredValueMeaning::Identity variant' is rejected; the variant is kept and made load-bearing —
the facts-ordering bug is fixed so Type::Identity members resolve (stores collected before
resources or an equivalent resolution pass), and stored_key's Identity arm decodes the identity
payload instead of returning None.

**5. Integrity completeness pass (II.D1).** Decided: accept. `marrow data integrity` gains a
completeness pass — the existing streaming cell walk grouped by entry asserts every member the
checked schema marks required has a cell, consuming CheckedSavedMemberKind::Field { required }
facts so requiredness keeps one semantic owner; the rule's scope follows language/02 exactly:
per existing record and per existing keyed-layer entry, unkeyed-group required fields count
toward the containing resource, and defaulted members carry no completeness obligation. Findings
are the typed `data.incomplete`, keyed by store catalog id, record identity, and missing member
catalog id — never source spelling — with exit 1. No implicit scan is added to `check --data`
and no check-side opt-in flag ships in v0.1 (deferred; moot since gate 25 retires the spelling).
Accepted storage-engine/05's finding list is amended (W1.1); data-tools.md/cli.md/error-codes.md
move in lockstep with the cli.md blind-spot caveat deleted; the conformance fixture is
maintenance-delete a required field, integrity exits 1 with data.incomplete, repair through
managed paths, passes.

### Group B — Durability, concurrency, transactions

**6. Commit posture (II.B1 item 3).** Decided: documented one-phase. Marrow keeps redb's
one-phase commit — no `set_two_phase_commit` call, no posture flag, no configuration — recorded
in a Durability and Recovery section of docs/backend-contract.md plus a one-sentence
storage-engine/02 amendment (W1.1), naming the residual risk exactly: a crash that tears the
commit slot is misread as committed only if the torn bytes collide with redb's non-cryptographic
slot checksum. Two-phase commit (`WriteTransaction::set_two_phase_commit(true)`, doubling fsyncs
per commit) is named in the same section as the escalation path if the W2.2 crash harness or
field evidence ever shows a slot misread; flipping it later is a runtime behavior change, not a
format break, so declining it now is not a keep-forever decision. (Durability pin, parent-dir
fsync, and crash harness are not decisions — they are P0 work in W2.2.)

**7. Read-only lock contract (II.B3).** Decided: amend the ADR to pinned behavior. W1.1 rewrites
storage-engine/01's read-only-lock sentence to: read-only opens coexist with each other; a
read-only open and a read-write open mutually exclude, in both directions, refused with
`store.locked` — and in the same pass adds the cross-process qualifier to foundations/02's
"many snapshot readers" sentence (it describes the engine's in-process snapshot model and the
deferred multi-reader handle, not concurrent OS processes; exactly one process opens the store
file at a time). error-codes.md's `store.locked` row becomes "The store file is held open by
another process (a writer or a read-only inspection)" plus the remedy (close the other process,
then retry); no writer-vs-reader holder detection is built (redb does not expose it). The
snapshot-copy open/copy/release inspection redesign is recorded as named-and-not-recommended for
v0.1; live inspection of a running application remains the deferred tooling/02 capability. W3.2
carries the implementation half: lock tests re-asserted against this contract, commit-id
allocation moved inside the write bracket, multi-reader handle model recorded as
designed-but-unbuilt.

**8. Savepoints — fix vs flatten (II.B5 + IV.D1(b) vs removal #11).** Decided: flatten
(removal #11). A nested `transaction` block joins its enclosing transaction; saved writes commit
only at the outermost block's clean exit; an error escaping any transaction block rolls back the
entire outermost transaction and propagates to the first handler outside the outermost block —
handlers between a nested block and that boundary do not intercept it (the only rule that never
commits more than savepoints would, with no doomed-transaction state machinery), while an error
caught before escaping any transaction block stays ordinary control flow. Delete the redb
pre-image undo journal entirely (including `mutate`'s `Vec<Undo>` return and the depth-1 case),
the MemStore savepoint clone stack, the nesting conformance laws, and the depth-tagged
TransactionState bookkeeping (`lower_savepoint_level`, depth-tagged required-entry checks and
pending stamps collapse to single-level state); W1.1 amends storage-engine/02's "Nested source
transactions are savepoints (or equivalent undo)" clause to the flat-join contract and names
savepoints as a purely additive future return on a real engine primitive; W3.1 rewrites
resources-and-storage.md lines 586-590 and backend-contract.md's savepoint section in lockstep
with the runtime change. The declined-branch fallback (II.B5 journal sink + IV.D1(b) bounded
journal) is void — both holes die with the journal; IV.D1(a) and (c) still execute in Wave 3.

**9. Activation receipt + atomic catalog commit (removals #24/#27 vs III.B3).** Decided: accept
both removals in order, with the III.B3 fold resolved to self-healing. Removal #24 lands first
(W3.4): delete the seven CommitMetadata per-effect evidence fields, their codec arms,
completion/{default,index,retire,transform,receipt}.rs and evidence.rs, keeping the slim
identity gate (proposal.rs + verdict.rs) and the in-memory ActivationReceipt for CLI rendering;
the backup CommitDescriptor drops the same seven fields under one FORMAT_VERSION 3→4 bump in the
same lane; the transform-reproof tradeoff is named in the lane doc and data-evolution.md's
slimmed activation-fencing paragraph; must land before gate 35 freezes storage-engine/04.
Removal #27 follows (W3.5): commit the accepted-catalog bytes in the same store transaction as
apply/auto-apply, making marrow.catalog.json an idempotent derived render (catalog-identity/02
preserved — the file remains the committed source-tree artifact, the store copy is the crash
bridge), and delete completion/, rebind_activation_resume_program, resume_completion, and the
evolve_cli_resume suite (replaced by one store-to-file recovery test), sequenced with — never
parallel to — the live engine-resident-catalog refactor, with W3.5 as its prototype-removal
ledger. III.B3 narrows to the fence-message audit inside W3.5, and its
guidance-only-vs-self-healing fork is decided as self-healing: any command finding the store's
committed catalog ahead of the file rewrites the file from committed store state and proceeds
(safe — a derived render, no evidence re-proving), so StoreEvolved stops prescribing recompile
unconditionally and keeps the recompile-or-upgrade message only for a genuinely newer-binary
store, with the remaining fence messages audited for actionable next commands. Removal #25
(pre-digest-stamp fence fallback) is confirmed accepted with its verified widened scope
(window.rs fence condition, preview.rs digest filter, the new
epoch_stamped_receipt_with_empty_digest_is_schema_drift test, the commit==None adoption path
untouched) and lands in W1.5.

**10. Meta-cell collapse Part B.** Decided: Part A only; Part B rejected. `layout_epoch` remains
a named field of CommitMetadata and EngineDescriptor in the format that freezes at gate 35, and
storage-engine/02's commit-metadata field list stands unamended; this is a deliberate
keep-forever choice under the subtraction window. Part A (cells 01-03 collapse into the one
commit-receipt cell, six facade methods deleted, ManifestCommitBindingMismatch deleted,
"stamped" = read_commit_metadata() is Some) executes in W3.3 as planned with no further
decision.

### Group C — Language surface: additions and closures

**11. Bounded ordered reads (II.C2).** Decided: accept bounded ordered reads with this exact
rule set. Disambiguation rule, recorded verbatim in the spec: in a for-head over steppable
scalars a range ENUMERATES values (optionally stepped with `by`); in key-argument position — an
index branch, store keyspace, or keyed layer — a range BOUNDS a stored scan over existing
entries, `by` is rejected there, and only the trailing supplied component may be a range (leading
components are exact values; a non-trailing range is a check error). Endpoints: single scalars of
the trailing component's declared type (no composite/tuple endpoints); any orderable key scalar
is rangeable (decimal stays excluded per language/04 and storage-engine/04; identity-typed
components from gate 4 take exact values only — no ranges over identity components in v0.1); both
`..` and `..=` are accepted; open forms `start..` and `..end`/`..=end` are accepted in
key-argument position only, while a fully unbounded bare `..` is rejected because the partial
application already spells it; a range argument is iteration, never lookup — it never satisfies a
unique index's fully-applied rule, never grants unique-lookup or point-read narrowing, and
yielded identities carry ordinary iteration-derived presence per gate 1(a); keyset pagination is
by typed key (pass the last-seen key as the next range start — no cursor, page, or resume objects
ever). The storage-engine/03 bounds sentence ("Bounds, windows, resume tokens, and checkpoints
are for platform surfaces..., not ordinary local loops") is amended in W1.1 to distinguish typed
source-level key bounds (allowed — they change which entries are in the iterable) from
iteration-state machinery (still excluded); the cost-model one-liner records that a range
argument is the same single range scan over that branch with explicit bounds readable off the
source; the decision and ADR amendment land now, implementation is lane W4.8 (the named
slippable language lane), and if W4.8 slips past the v0.1 tag the syntax stays reserved (grammar
parses, checker rejects), docs/language documents nothing until the lane lands, and no other
feature may claim `..` in key-argument position.

**12. `??` precedence (II.E1 b).** Decided: accept — `??` moves from its current level 8
(between comparison and equality) to its own non-associative level immediately looser than
additive and tighter than ranges, taking the ladder slot vacated by the deleted `_` concat
operator, so `count ?? 0 < 5` parses `(count ?? 0) < 5`, `start ?? 1 .. n` parses
`(start ?? 1) .. n`, and `x ?? y + 1` stays `x ?? (y + 1)`. Non-associativity is preserved
exactly as today (`a ?? b ?? c` rejected) and grammar.md records it as a deliberate design
choice with layered defaults as the named extension point (the C40 sentence W4.1 already
carries); the documented "binds tighter than `==`" invariant remains true. parse_expr.rs and the
hand-mirrored binary_precedence table in format.rs move in lockstep, with syntax.md's operator
table and grammar.md's coalesce_expr/range_expr productions updated in the same change.
(`_`→`+` concat is already user-approved; it executes, not re-decided.)

**13. Output builtin + print argument contract (removal + II.E2 item 2).** Decided: delete
`write(...)` and keep `print(...)` as the sole output builtin, always appending a trailing
newline; II.E2 item 2's fork is resolved by documenting the actual runtime contract as v0.1, not
string-only-plus-checker-rule. The print contract, recorded in the spec: print renders string,
int, bool, decimal, and identity values (a single-key identity renders as its key, a composite
as `identity(k1, k2)`); every other value — instant, date, duration, bytes, enum, sequence,
local tree, resource — raises the fatal `run.unsupported` runtime fault, with temporal values
rendered explicitly through std::clock::formatInstant/formatDate/formatDuration; no new checker
rule in v0.1. Mechanics per the verified removal: OutputKind and CheckedBuiltinCall::Write are
deleted, the write_does_not_add_a_newline test is deleted, ~41 doc examples and ~25-30 embedded
test strings migrate mechanically to print, the grammar.md Ambiguity Rules sentence and the
syntax.md/README dual mentions name only print, and the future partial-line use case is not
pre-reserved as std::io streaming — if needed it lands in std::io at design time with a real
contract.

**14. entries() closure (II.E3).** Decided: accept the closure in one motion — (1) bare two-name
loops over local keyed trees bind key and value, matching the saved-layer rule and deleting the
runtime "use entries(...)" directive; (2) `entries(...)` is restricted to two-name for-loop-head
position, including under `reversed(...)`, enforced by a checker diagnostic — expression position
AND single-name loops over `entries(...)` are both check errors; (3) the runtime
pair-materialization paths are deleted so `Value::Sequence([key, value])` is never observable
anywhere. Keep-or-cut: KEEP `entries()` as documented loop-head sugar — ADR language/07's
enumerated builtin list stays unchanged, with builtins.md and control-flow-and-effects.md
updated in the same docs motion as the II.E5 local-iteration paragraph.

**15. Visibility contract (II.E4).** Decided: option (a) — spec the current behavior honestly,
no ADR change. The spec states visibility per declaration kind: functions are pub-gated; enum
`pub` is enforced cross-module; resources take no marker and are project-visible by qualified
path; top-level constants are always module-private; and one new sentence states that in v0.1
any module may read and write any saved root. The asymmetry is named a v0.1 simplification with
docs/future's uniform-pub contract linked as the planned tightening (under which `pub` on a
saved resource will govern ^-root access); the stale ast.rs:376 enum-enforcement comment
(carried by W1.4) and the resolve.rs comment (W4.6) are fixed. Options (b) constants-take-pub
and (c) uniform-pub pull-forward are rejected for v0.1, with (b) explicitly recorded as still
available post-v0.1 since adding a `pub` marker to constants later is additive and non-breaking.

**16. Stdlib floor + indexOf shape (II.E6).** Decided: accept all 15 ops through the StdOp
descriptor table with these exact signatures — std::text::{slice(value: string, start: int,
end: int): string, startsWith(value: string, prefix: string): bool, endsWith(value: string,
suffix: string): bool, indexOf(value: string, needle: string) → maybe-present int,
replace(value: string, from: string, to: string): string, join(parts: sequence[string],
separator: string): string, toUpper(value: string): string, toLower(value: string): string} and
std::math::{minInt(a: int, b: int): int, maxInt(a: int, b: int): int, minDecimal(a: decimal,
b: decimal): decimal, maxDecimal(a: decimal, b: decimal): decimal, round(value: decimal): int,
ceiling(value: decimal): int, powInt(base: int, exp: int): int with checked overflow per the
existing numeric-error pattern}. indexOf takes the maybe-present amendment: the call is a
maybe-present read site resolved exactly like next/prev (`??`, exists-style guard),
language/02's closed read-site list gains a declared-maybe-present-result category and
language/07's concrete-signatures framing is amended in lockstep (W1.1), and the ReturnType
maybe marker is built as a general function-signature-level fact, never a builtin-descriptor
special case (the gate-39 foundation rider holds regardless of gate 39's outcome); the -1
fallback is declined. `round` mode is half-to-even; all text ops index in Unicode scalar values
consistent with the documented length semantics; case conversion is Unicode simple case mapping
with an explicit no-locale contract; sqrt is excluded with the stated no-exact-decimal-form
reason; join's sequence-typed ParamType variant lands in W4.5; and std_pure.rs's parallel
string-matched dispatch converges onto the descriptor table so dispatch has one owner, every
signature reviewed against the no-overloading law (hence the Int/Decimal name pairs).

### Group C′ — Language surface: subtractions (pre-release-only window)

**17. Cut concise saved-resource declaration sugar (removal #3).** Decided: cut the `at`
declaration form entirely; the split `resource` + `store ^root(keys): R` form (indexes declared
under the store) is the only declaration spelling. Delete the Keyword::At resource production,
parse_decl/head.rs path, schema tests for the removed resource/store sugar, formatter support, the grammar.md:502-504
in-resource-index ambiguity rule, the resources-and-storage.md namespace caveat (282-284) and
'is accepted as declaration sugar' passage (214-216); rewrite all ~757 test occurrences in ~130
files plus the fixtures/v01 corpus mechanically to the split form. The catalog-identity/01
line-27 'is sugar that desugars to' bullet is amended via W1.1 as an explicit recorded revisit
(not a silent edit): the split form is the sole spelling and the resource/store separation is
now also a single-spelling fact. Saved bytes are unaffected (sugar desugared pre-catalog);
W1.7's byte-identical-catalog review check stands.

**18. Cut loop labels, finally, and phantom `out` (removals #7, #8, #9A).** Decided: cut all
three under one ADR language/06 amendment (W1.1) that records each rationale and explicitly
states the grammar tightening 'a try block must always have a catch clause'. Labels: delete the
grammar.md loop_label production, the Loop Labels spec section, try_loop_label, AST/IR label
fields, format_label_prefix/suffix, collapse jump_resolves_in_scope to depth-only, drop label
params from loop_exec; replacement idiom is function extraction with return. Finally: delete the
runtime statement.rs finally execution and all checker finally modules; error-codes.md row
check.finally_control_flow is deleted outright (no future/reserved placeholder — the code
contract freezes at release, not before); check.try_handler rewording to 'A `try` block has no
`catch` clause' lands in error-codes.md:161 plus diagnostics.rs and rules.rs in the same change;
replacement idiom is catch-cleanup-rethrow, with reintroduction only when a handle/capability
surface creates real cleanup obligations. `out`: delete Keyword::Out from token.rs:201 and the
two parser branches (parse_expr.rs:356, parse_decl/params.rs:140) with their diagnostics, and
drop `out` from the syntax.md reserved-word list entirely — fully unreserved, no merge/lock-style
note, superseding REPORT II.E2 item 1's keep-it-reserved sentence (that guidance assumed the ADR
still claimed `out`); `finally` and label syntax likewise unreserve fully.

**19. Cut inout parameter mode (removal #9B).** Decided: cut inout now. Amend ADR language/06
(W1.1, same amendment as gate 18) to one parameter mode — read-only by-value parameters —
striking the inout clause alongside the out clause; `inout` is fully unreserved. Delete the
param/arg mode parsing, CheckedArgMode/CheckedParamMode from the checked IR, the Place type and
call_args.rs write-back path, the checker writable-place rules (rejected_surface.rs,
checks/calls.rs), and the presence-narrowing expiry logic it forces; rewrite the ~80 inout test
references to return-value style (`book = normalize(book)`). Replacement contract recorded in
the spec: return-value style for local mutation helpers; explicit saved assignments at call
sites for saved data (already the rule).

**20. Cut decimal range steppables (removal #10).** Decided: cut decimal from range-loop
steppable types; int, date, and instant ranges are unchanged. The ADR language/06
range-enumeration amendment lands in the same commit as the code change (checker ranges.rs
decimal arms: literal_decimal_value, its literal_step_sign use, the Decimal check_step_type arm,
Decimal removed from is_steppable; runtime RangeIter::Decimal + decimal_range_iter in
loop_exec.rs; the 8 marrow-check/tests/ranges.rs decimal cases and the eval_basics.rs decimal
case; control-flow-and-effects.md decimal paragraph; syntax.md:243 steppable list; the
error-codes.md check.range description). The spec's replacement note uses real Marrow syntax
only: `var x: decimal = decimal(i) * 0.25` inside an int loop (or `decimal(i) * step` for a
variable step) — never the fictional `then`/`as` spelling.

**21. Quoted field segments, Phase 1 (removal #4).** Decided: accept Phase 1. Quoted field
segments are rejected unconditionally at parse time — no parse-time maintenance context exists
in v0.1 — with a parse diagnostic stating quoted segments are not part of the ordinary
expression grammar and naming operator maintenance mode (ADR language/03) as their decided home;
grammar.md's field_name drops `| string_lit`; the syntax.md:319-329 and
resources-and-storage.md:286-288 explanatory passages are deleted; presence keys drop the quoted
flag from key text (keys.rs:143/152 become `field:{base}:{name}` / `optional:{base}:{name}`).
The AST/CheckedExpr `quoted: bool`, rejected_surface.rs guard, formatter quoted-emission, and
the evolution/intents + durable_path plumbing are retained inert for Phase 2, whose deletion
remains explicitly blocked on the maintenance-surface syntax design (the one by-design deferral
in this gate). Tests: scenario_lang_db_seams.rs:213 deleted as vacuous;
parse_paths_calls.rs:130/:223 repointed to assert the parse error; and — correcting the
removal's phase scoping — the quoted-source eval_maintenance.rs tests are deleted in Phase 1
(their `"old-title"` sources no longer parse anywhere), with the bare-segment maintenance tests
kept; the ADR language/03 maintenance guarantee stays a designed-but-unbuilt contract until
Phase 2.

**22. Cut map[K, V] saved-member sugar, keep sequence[T] (removal #5, Change B).** Decided: cut
the map[K, V] saved-member sugar; keep sequence[T]; record the asymmetry argument in the
spec/lane: sequence[T] is a stdlib return-type contract (std::text::split returns
sequence[string], verified in stdlib.rs) with no equally-ergonomic explicit spelling for
positional data, while map's explicit keyed-leaf form `scores(key: K): V` is equally readable,
is what every teaching page uses, and is the identical schema the sugar desugars to (stored
bytes unaffected). After the cut `map[...]` is no longer a type spelling (parse error in type
position), so schema.unsupported_type is retired: both constructors in marrow-schema errors.rs
are map-only — delete them and the docs/error-codes.md:194 row (pre-release; the code contract
is not yet frozen). Full blast per the trimmed removal: sequence desugar path retained intact;
leaf_type.rs and discharge/leaf_obligations.rs comments referencing the map spelling cleaned;
contains_map_type guard in driver.rs and the token.rs map_type_parts guard deleted; grammar.md
map_member_type production plus types.md/data-evolution.md prose removed. Change A (future-page
scope trim) rides W1.2 as doc-only, written against this outcome: the docs/future
Collection-spellings section describes the entire map/set family — local map/set values,
insert(path), set[K], AND the map[K, V] saved-member spelling — as unbuilt future surface
returning whole later; it must not state the saved-member spelling is shipped.

### Group D — Tooling and operator surface

**23. Delete `marrow serve` + the in-tree `marrow lsp` (removals #12, #13) vs REPORT III.D4
hardening.** Decided: delete both `marrow serve` and the in-tree `marrow lsp` in full at P1
(removals #12 + #13), rejecting the partial keep-debug_data_roots/walk fallback; REPORT III.D4
is retired as moot — all three of its findings die with the deleted surfaces. The `protocol.*`
code family and its kind row leave error-codes.md; docs/serve-protocol.md, docs/lsp.md, and
docs/implementation/serve-lsp.md are deleted; docs/future/serve-protocol.md is deleted outright
with no replacement stub — the designed-contract supersession is approved, and Deferred ADR
tooling/02 is recorded as the sole owner of any future app-server/local-API contract. marrow-lsp
(direct crate linkage), `marrow data`, and the analysis API are the named replacements; the
entire analysis API surface (analyze_project, ProjectSources overlays, AnalysisSnapshot, cursor
queries, BindingIndex) is untouched.

**24. Format matrix + CI test output conflict; error-code trims.** Decided: III.C2 wins and
removal #14 sub-lane B is dropped — `marrow test` keeps `--format` and gains real machine
results: `--format json|jsonl` emits one pre-stable record per test ({name, outcome, code?,
file?, span?}) plus a summary record (text default unchanged, hard-coded Text diagnostic paths
fixed in the same change), and `--filter <substring>` matches qualified test names with zero
matches failing closed as `test.none` naming the filter (summary reports selected/total; no tags
or markers in v0.1). Sub-lane A is accepted — trace is text-only: TraceHook's records buffer,
all Json/Jsonl trace arms, and write_target_json are deleted, `--trace` combined with
`--format json|jsonl` is a usage error, and `jsonl` is removed from `run` entirely, leaving
`run --format text|json` valid for `--dry-run` and, once gate 27 lands, the plain-run invocation
envelope; sub-lane C is accepted — `backup`/`restore` lose `--format`, with exit codes plus
stable stderr codes as the scripting contract. III.C1's honor-format-on-every-path rule applies
to every surviving format-bearing command, routed through load_checked_project_with_format
including failure paths, with one broken-marrow.json JSON-envelope test per command. Error-code
trims per removal #15: all three drift codes — `evolve.drift`, `evolve.store_commit_drift`,
`evolve.plan_mismatch` — collapse into one `evolve.drift` whose `data.drift_kind` is `witness`,
`store_commit` (payload `pinned`/`found`), or `plan_mismatch` (payload `expected`/`staged`),
render-only with the ApplyError variants unchanged; `run.schema_drift` stays a distinct runtime
fence code; the `capability` kind row and its footnote are deleted from error-codes.md, and the
exit-code-1 prose drops `capability` as a kind-style word in favor of the plain phrase 'host
capability'.

**25. `check --data` collapse into evolve preview (removal #18).** Decided: retire the
`check --data` spelling now — the prior success-envelope question is resolved by retiring the
{kind: data_check, status: ok, records_scanned} envelope with it; `marrow evolve preview` with
its existing full envelope is the one operator surface and grows no abbreviated mode.
`check --data` becomes an ordinary unknown-flag usage error with no alias, fallback, or
deprecation shim; the `run.store_unstamped` instruction (cmd_run.rs:281 and its error-codes.md
row) is rewritten to name `marrow evolve preview` and `marrow evolve apply`, atomically with the
deletion alongside cli.md and the data-evolution.md references, and data_orphan_cli.rs migrates
to assert the evolve-preview envelope through the production pipeline. ADR model-ir/02 is
amended with the explicit argument 'the data-attached obligation-discharge stage survives; the
duplicate CLI spelling is removed' (the wording rides W1.1).

**26. Dry-run classification (III.B4).** Decided: confirmed — `--dry-run` is a preview surface
classified with check (read-only, non-committing): it skips commit_pending_identity (reporting
'would freeze durable identity (epoch N)') and skips auto-apply (on FenceError::SchemaDrift
reporting 'would auto-apply: <obligation summary>' plus an explicit sentence that a real run
would proceed, then not executing), and executes against the store as-is with the rolled-back
savepoint only when the fence would pass; would-freeze/would-apply lines are report content, not
errors, so a successful preview exits 0. The stated contract is durable-identity stability — no
store cells, no epoch stamp, no catalog file — not store-file byte identity
(dry_run_does_not_promise_native_file_byte_stability stays deliberate), pinned with the
check_read_only_cli.rs pattern; docs/cli.md and data-evolution.md's Run-Time Auto-Apply section
are updated, and the report names `evolve preview` as the surface for previewing the post-apply
program against a drifted store. This composes with the locked catalog decision (identity
commits on run/apply; check and inspection are read-only) by assigning dry-run to the check
side.

**27. Typed entry invocation argument surface (III.E1).** Decided: accept — the surface is
repeatable `--arg name=value` with exactly one occurrence per non-sequence parameter (duplicate
names, unknown names, and missing required parameters are typed errors); the `--args-json`
alternative is rejected as a second input grammar. Textual forms, specified in cli.md in
lockstep with modules-functions.md: int/decimal/bool use the language literal grammar
(`true`/`false`); string is the raw remainder after the first `=` with no quoting or escape
processing; bytes uses the language byte-literal form `b"..."`; date is `YYYY-MM-DD` and instant
is RFC 3339 `YYYY-MM-DDTHH:MM:SSZ` (the canonical saved textual forms in types.md); duration
uses the language duration-literal grammar; enums take the bare member name validated against
the declared enum type; sequence parameters (element type scalar or enum only) repeat the flag —
one element per occurrence in argv order — with `[]` as the single-occurrence empty-sequence
spelling, and nested or identity-element sequences excluded. Identity parameters decode via the
gate-2 `Id(^store, key...)` shape with provenance from the checked signature: a single-component
key takes the bare component textual form (`--arg id=42` against `Id(^books)`), composite-key
identity parameters are excluded in v0.1 with a typed error naming the wrapper-entry pattern,
and constructed identities carry no presence proof per gate 1c. Two new runtime codes carry the
contract — `run.entry_surface` (signature outside the v0.1 surface:
resource/local-tree/nested-sequence/composite-key-identity parameters, and resource-shaped
returns when `--format json` is requested) and `run.entry_argument` (a supplied --arg fails
decoding), both kind runtime, exit 1, rendered machine-readably in the envelope — with all
decoding in marrow-run::entry behind the CheckedEntryCall seam (G1-2) and the CLI parser plus
JSON envelope as thin adapters; under `--format json` stdout is one pre-stable JSON envelope
carrying the typed return (identity values rendered in the same JSON form `marrow data` uses)
plus the captured print output, text runs keep today's discard contract, and cli.md states the
per-invocation check+open cost so the CLI is not benchmarked as a server.

**28. Call-depth budget (IV.A1): constant, naming, classification, nesting bound.** Decided:
accept as framed with the constants pinned — `CALL_DEPTH_BUDGET: usize = 256`, a named constant
in marrow-run with no configurability, enforced in invoke_function where depth already
increments, raising a fatal, uncatchable `run.depth` fault (kind runtime, exit 1) carrying the
call span and function name; a fault inside a transaction rolls back through the normal fault
path. The value 256 is provisional under a no-sitting evidence rule: the W2.3 headroom test runs
full-budget recursion in a debug build on a spawned thread with a 2 MiB stack (recorded in docs
as the minimum supported execution stack); if peak use exceeds half that stack, the lane halves
the constant to the largest power of two giving at least 2x headroom, floor 64 — below 64 the
lane fixes per-activation Rust frame cost instead of shrinking further — and the constant is
never raised above 256 in v0.1. Syntactic nesting (expressions and nested statement blocks) is
bounded inside the marrow-syntax recursive-descent parser by
`PARSE_NESTING_BUDGET: usize = 256`, reported as the existing `parse.syntax` diagnostic — no new
parse code, keeping error-codes.md's 'the only parse.* code' sentence true — which transitively
bounds lowering recursion and per-activation expression evaluation. error-codes.md gains the
`run.depth` row, the docs/language budget note documents both constants, and the done criterion
stands: no input in the fuzz-ish corpus aborts the process.

**29. Backup overwrite flag (III.A1 rider).** Decided: no overwrite flag of any spelling (no
--force, no --overwrite, no --no-clobber). `marrow backup <projectdir> <output-file>` replaces
an existing file at the output path only by atomic rename after the new artifact is fully
written, flushed, and fsynced (the W2.4 temp+rename contract); on any failure the prior file is
byte-identical and no temp file remains. cli.md states this replace-only-on-success contract in
one sentence.

**30. `marrow init` scaffold default (III.C3 rider).** Decided: ratified — the scaffold's
marrow.json uses `"store": {"backend": "native", "dataDir": ".marrow/data"}`, written explicitly
(consistent with gate 42's required store block), with the full scaffold config pinned as
{sourceRoots:["src"], run.defaultEntry:"<name>::books::main", store as above, tests:["tests"]} —
the tests entry is the plain directory path per gate 31, superseding III.C3's `tests/**/*.mw`
spelling. The scaffold mirrors the quickstart verbatim as one drift-pinned fixture; no
.gitignore or other extra files in the v0.1 scaffold (quickstart has none, and the one-fixture
pin is the review criterion).

**31. Tests config grammar (removal #17 + III.C2 c).** Decided: accepted. Each `tests` entry is
a plain under-root relative path: a directory is walked recursively collecting `.mw` files, a
file is taken directly; all three glob tails (`/**/*.mw`, `/**`, `/*.mw`) are deleted and
test_pattern_base goes with them. Fail-closed validation at config parse: any entry containing
one of the metacharacters `*`, `?`, `[`, `]`, `{`, `}` is `config.invalid`; the existing
under-root rule (no empty, absolute, or `..` entries) is unchanged. A well-formed entry whose
path does not exist contributes no tests and is not an error, with `test.none` remaining the
zero-tests-overall backstop; the symlink-skip rule is documented in project-config.md;
tooling/04's "array of glob patterns" wording becomes "array of directory or file paths"
(wording-only ADR touch, riding the W1.1 batch).

**32. acceptedCatalog config key deletion (removal #41).** Decided: accepted. Delete the
`acceptedCatalog` key entirely (marrow-project config struct field,
ConfigPathField::AcceptedCatalog, parse and under-root validation arms); the catalog file is
fixed at `marrow.catalog.json` in the project root (the current code default). The
deny-unknown-keys expected-list shrinks accordingly; the three config tests using a non-default
value are rewritten; docs/project-config.md (rows at 21, 46, 129, 179, 205) and
data-evolution.md:159 drop the key. This aligns with tooling/04's locked key list, which never
included acceptedCatalog.

**33. Distribution story (III.B2 item 5).** Decided: cargo-install-only for v0.1, stated as a
named limitation: the channel set is (1) a tagged source release `v0.1.0` on the GitHub repo and
(2) clone + `cargo install --locked --path crates/marrow` (install.md gains `--locked` to honor
the committed Cargo.lock); no crates.io publication and no prebuilt binaries in v0.1 — both
named in stability.md/install.md as the fast-follow, and install.md's speculative Release
Package Shape section dies with removal #23. Platform statement: unix-only (Linux and macOS);
the non-unix entropy panic in stable_id.rs is resolved as a documented constraint, not a runtime
diagnostic — the `#[cfg(not(unix))]` panic stays as the backstop for out-of-contract builds,
stability.md and install.md state the supported-platform set, and install.md's Windows
binary-path sentence is deleted.

### Group E — Decision-record hygiene (one approval batch)

**34. ADR truth batch.** Decided: approved as one batch, with W1.1's absence scan as the
authority over the enumerated lists: strike every normative present-tense `edit`-verb mention
(foundations/01:41, storage-engine/02:8/43/45, catalog-identity/01:76, engineering-style/02:46,
PLUS the three sites the removal entry missed — storage-engine/01:42, model-ir/01:50, and the
language/README:10 index line) and every assertion-write mention (foundations/01 law 7,
storage-engine/02:8/43/48, model-ir/01:52, engineering-style/02:47-48), demoting
language/03:31-32 to an explicitly future/reserved note; replace catalog-identity/01's alias
paragraph (lines 31-37) with one sentence deferring to language/05, and rewrite language/05:17
to plain `Id(^authors)` and language/04:45's example to `Id(^books)`. Citations: re-point all
`marrow/docs/implementation.md` citations to `marrow/docs/implementation/`; drop every
`docs/roadmap/` citation outright rather than re-pointing (the collision resolution stands —
W1.2 deletes that directory), and delete any Sources line left empty by the drop
(foundations/02, engineering-style/01). Law count: foundations/README.md changes 'fourteen' to
'sixteen production-foundation laws' (verified: foundations/01 states 16). Supersession
metadata: one `Amended: YYYY-MM-DD — <what changed; owning ADR>` line directly under the
`Status:` line of each touched ADR, later amendments appended in date order, with template.md
documenting the optional line once.

### Group F — End-gates

**35. storage-engine/04 → Accepted (evidence protocol).** Decided: protocol fixed now; the flip
is pre-authorized and needs no further sitting. Evidence packet: the freeze-gate ledger (started
in W2.2) holds one entry per family storage-engine/04 names — conformance (17-law suite plus
reverse/bounded-scan, identity-component, and CommitId-density laws; W2.8/W3.2/W4.7/W4.8),
ordering (key-codec property round-trips plus byte-fingerprint goldens; W2.2/W2.8),
crash/recovery (kill-during-commit plus torn-write reopen; W2.2), backup (atomic write,
all-or-nothing refusal, epoch-mismatch restore refusal; W2.4/W3.8), evolution (multi-epoch
lifecycle; W3.8) — each entry naming its suites, producing lane, integrating commit, reviewer
verdicts, and one fresh clean-worktree full-workspace run at W7.2 assembly with explicit
CARGO_TARGET_DIR; W2.9 hostile-input results ride as supporting evidence, not a sixth gate.
Green threshold: every fixture in all five families passes in that single fresh run with zero
`#[ignore]` members inside the families; deterministic kill points pass 100% and the bounded
soak shows zero both-or-invisible violations — a reproduced violation is red, an unreproduced
environmental flake is investigated and rerun, never waved through; if W4.8 (the named slippable
language lane) slips, its bounded-scan laws follow it and do not block the freeze, since
read-path laws add no bytes. On all-green, W7.2 flips storage-engine/04 to Accepted in
marrow-decisions with an appended Evidence note (date, families, suite names, marrow commit
hash) and LayoutEpoch 0 is frozen — every later byte-format change is a LayoutEpoch recompile,
never an in-place edit. Any red family: no flip, no freeze, and the v0.1 tag is blocked; a
defect lane fixes it, the entire family re-runs fresh (not just the failing case), and the
packet is re-assembled — byte changes made by the fix are permitted precisely because nothing is
frozen until the post-fix green packet.

**36. foundations/03 self-deletion timing.** Decided: approved with a single trigger: the v0.1
tag — the alternative completion-note path is dropped, since the tag itself is the completion
declaration and one trigger leaves no ambiguity. Execution: one marrow-decisions commit, made
immediately after W7.4 pushes the v0.1 tag, deletes
adr/foundations/03-prototype-status-and-replacement.md and removes its index entries from both
adr/foundations/README.md and adr/README.md, with the commit message naming the v0.1 tag; no
content migration (the crate-ownership map is already owned by docs/implementation/README.md's
Crate map section, verified present). The deletion commit hash is recorded in the W7.4 release
evidence packet; the file is never deleted before the tag exists. W1.1 still applies the gate-34
Sources fix to foundations/03 in Wave 1 — it remains a live decision record for the entire build
period.

**37. tooling/05 export boundary — ordered-profile application-boundary decision (G1-6).**
Decided: Profile A (linked-Rust embedding) is first and Profile B (export fn + generated
TypeScript) is second, both post-v0.1 over the same W6.10 session and W6.6 envelope types; the
ordering is final now and is not reopened at the checkpoint — what remains post-v0.1 is only
scheduling Profile A's implementation lane. Profile A's preconditions are W2.3 containment and
W3.2 thread-confined sessions; it requires no codegen, no transport, no principal design, and no
ADR amendment (tooling/05's limited-to-local-embedding-pending-auth Consequences clause is the
authority), and it ships only as the single G1-5 facade surface, never the internal crates.
Profile B is full tooling/05 acceptance: the same boundary plus the W5.5 effect classifier, the
request/principal design, and exported-function identity stabilization (the gate-48/C10 digest
model) settled at that acceptance; III.E1's typed entry invocation is the named interim contract
until a profile ships. The stability.md sentence, landed via W7.2, reads exactly: 'The
application boundary opens in ordered profiles: first linked-Rust embedding — an in-process
library API over the run session and result envelope, resting on host containment and
thread-confined sessions, with no codegen, no transport, and no principal model — then
`export fn` with generated TypeScript over the same boundary; until a profile ships, typed entry
invocation (`marrow run` with `--arg` and `--format json`) is the supported integration
surface.' No implementation and no ADR amendment in v0.1.

### Group G — Innovation

**38. Binding existence guard spelling (C35).** Decided: option (a) — the binding existence
guard is spelled `if const x = place`. The if-head production is
`"if" "const" identifier "=" <maybe-present read>`, composing with the existing else-if/else
clauses; the bound name is immutable, scoped to the guarded block, discharged via
PresenceProofSource::Narrowing, and costs exactly one priced point read. language/02's
`if let x = place` spelling is amended to `if const x = place` in W1.1; the form is built inside
W2.7 while the presence machinery is open. Options (b) `if let` and (c) decline are rejected.

**39. Maybe-present user-function results (C36).** Decided: accept both spellings. (a)
Signature: `maybe T`, valid only in the function_decl return_type position
(`return_type = ":" "maybe"? type`) — never a field, parameter, stored, nested, or keyed-tree
type, enforced by grammar position plus checker; the call expression is a maybe-present read
site resolved by the existing machinery (`??`, the gate-38 guard, `exists`, `?.`). (b) Body
exit: `return absent`, with both `maybe` and `absent` added to the reserved-word list (W4.6
bookkeeping); `absent` is valid only as the entire return expression inside a `maybe T`
function; a maybe-function body may also `return <maybe-present read>` to propagate, resolved at
the caller's call site; implicit fall-through remains a missing-return error (absence is always
explicit), and no Option value exists at runtime. The ReturnType marker is a general
function-signature fact, never a builtin-descriptor special case; grammar, descriptor attachment,
the absent-exit form, and caller-site resolution share the existing maybe-present machinery.

**40. Temporal arithmetic operators + std::clock::add deletion (C41).** Decided: option (a) —
accept exactly the five typed operator rules — instant−instant→duration,
instant+duration→instant, instant−duration→instant, duration+duration→duration,
duration−duration→duration — with no implicit conversions, no date operands, and no
duration×scalar forms; everything outside the five rules is a type error. Overflow raises a new
catchable `run.temporal_overflow` (the run.decimal_overflow per-family pattern), raised when a
result leaves the representable envelope the saved encodings define (instant: the RFC 3339 saved
range; duration: i128 nanoseconds). `std::clock::add` is deleted in the same W4.1 change with
grep-zero across code and docs; calendar math (the addDays/daysBetween family) remains
std::clock's domain — operators own linear span math only. The language/04 operator-table
amendment rides W1.1. Options (b) operators-without-deletion and (c) decline are rejected.

**41. Typed keyed-layer entries (C34).** Decided: accept with the exclusion:
`comments(seq: int): Comment` is legal — a declared resource as a keyed layer's entry type, with
whole-entry writes validating required fields, whole-entry reads materializing like store
records, write planning over W2.1's record-body plan + entry-node cell, and two-name loops
binding typed entries (after W4.4); the anonymous indented entry form remains as inline sugar.
The explicit exclusion is part of the decision: the entry type is not an evolution target in
v0.1 — transform/default/retire addressing inside the layer fails closed exactly as today's
NestedRetireUnsupported, pending II.D2, and the resources-and-storage.md lockstep section states
the write contract, read contract, and evolution boundary. Lane W4.12 executes as written,
including the identity/write-path soundness re-review-until-clean requirement.

**42. Required store block (C56).** Decided: accept — omitting the store block in marrow.json
becomes `config.invalid` with a two-choice diagnostic naming both backends by their exact config
spellings — `"native"` and `"memory"` — and showing that in-memory stays one explicit line
(`"store": { "backend": "memory" }`). The tooling/04 default-to-memory clause ('defaulting to
in-memory when store is omitted') is amended to silence-is-an-error in W1.1; the validation arm,
store-required tests, and two doc pages ride W6.7. Gate 30 is untouched: `marrow init` scaffolds
`native`, so the happy path never sees the diagnostic.

**43. Sensitive-data reservation (C05).** Decided: accept the reservation: `sensitive` and
`declassify` are added to the reserved-word list via W4.6's bookkeeping; `sensitive`'s grammar
position is documented as a schema member modifier in the `required` slot family, and any v0.1
use of either word is a parse-time rejection pointing to the future flow-policy contract. The
v0.1 output sink set is pinned in spec as exactly: `print`, every `std::log` function, and
`std::io::writeText`/`std::io::writeBytes` — the complete v0.1 egress set, growable only by spec
amendment. `declassify` is documented as the sole future auditable escape. No taint pass ships
in v0.1.

**44. No-triggers doctrine + `journal` reservation (C20).** Decided: option (a) — accept. One
doctrine paragraph lands on the docs/future contract page — 'Marrow never ships triggers,
observers, or a reactive engine; all reactivity is in-plan declared derivation or explicit
consumer code over a future journal primitive' — and `journal` is added to the reserved-word
list via W4.6. The `journal changes` feature itself stays deferred per the REPORT-Appendix
rejection of new durable journal surface in v0.1, until LayoutEpoch 0 freezes and II.E8/II.D2
resolve.

**45. Backup lineage (C52).** Decided: option (A) — reserve `parent_snapshot_digest` in the
backup-manifest codec as an always-empty typed sentinel, riding W3.4's existing FORMAT_VERSION
bump, marked in stability.md (W7.2) as reserved/semantics-undefined. The reservation is
fail-closed: v0.1 manifest validation rejects an artifact carrying a non-empty value as an
invalid manifest, so undefined lineage semantics can never be smuggled through a v0.1 reader.
Full backups remain the only v0.1 behavior; incremental-chain semantics are a future
FORMAT_VERSION-compatible addition.

**46. Unknown-tree consumption contract (C53).** Decided: accept; all sub-questions settled
now, implementation rides the II.E7 fast-follow against this frozen contract with the
evolution/integrity leaf-decode family as the one validity owner. (1) Spelling:
`Book(from: tree)` — the decode form is a resource-constructor call with exactly one argument
labeled `from` whose static type is `unknown`; since `unknown` is never a legal resource-member
type, the form is statically disjoint from field-population constructors, and
`decode(Book, tree)` is rejected. (2) Extra members fail closed; (3) the error-code family is
`decode.*`, kind runtime, catchable, every error carrying the failing path from the decode root
as a typed payload (prose render-only), with four rows reserved now under error-codes.md's
Deferred Surfaces: `decode.shape` (node-kind/structure mismatch), `decode.unknown_member`,
`decode.required_absent`, `decode.value` (leaf not a canonical form of its declared type).
(4) JSON null and a missing member both map to structural absence — absent on sparse members,
`decode.required_absent` on required ones; identity members decode their declared key components
with provenance supplied by the declared `Id(^store)` member type, exactly the W2.6 constructor
/ III.E1 boundary rule; int/decimal accept only the saved-encoding canonical forms; numeric
overflow (outside 64-bit int or the 34/34 decimal envelope) is a typed catchable `decode.value`
error at the failing path. Traversal mechanics beyond the six settled items (keyed-layer key
decoding, sequence element rules) are by design the II.E7 fast-follow's implementation scope
against this frozen contract — named scope, not an open decision.

**47. Effect-proven read-only opens (C06 + G1-4).** Decided: accept the seam exactly as scoped:
W5.5 lands the transitive, lowered-IR-resolved `write_effects_reachable` fact and the
effect-closure query on the analysis API (doubling as tooling/05's future export classifier),
with the named conformance test that a read-only-closure entry executes correctly against a
read-only TreeStore. The open-mode downgrade contract — an entry whose transitive closure proves
no reachable write effects may open the store read-only under the gate-7 lock contract, and the
first run against a store with no frozen catalog identity is always write-capable (identity
freeze writes) — is recorded as a section in docs/backend-contract.md (rides W5.5) plus one
amendment paragraph in storage-engine/01 beside the gate-7 lock amendment (rides W1.1); no new
ADR file is created. cmd_run wiring remains a named post-v0.1 follow-up under the existing C06
post-v01-seam entry.

**48. Function ABI identity (C10).** Decided: a fixed definition plus an explicit conditional
drafting step, so no further sitting is needed. Fixed now: exported-function durable identity is
a sha-256 digest (the existing source-digest/canonical-render-plus-digest pattern) of the
canonical rendering of the typed signature normal form, whose inputs are exactly parameter and
return types rendered nominally by catalog id (never source spelling), the effect class, and the
declared error surface under C07's raises model; the W6.6 envelope reserves one nullable field
spelled `signature_digest`, null throughout v0.1; no committed artifact and no function table in
v0.1. Conditional (the by-design evidence-dependent part): the precise normal-form rendering
text for the tooling/05 ADR revision is drafted at the Wave 5 wave-gate recording, after
W5.3/W5.4 land, as a pure byte-stable function of checked facts following the W5.4
discharge-digest pattern, reviewed in-lane with no user sitting; the digest is never emitted
(the field stays null) until both that rendering and the C07 raises model exist, and if
W5.3/W5.4 change the workspace digest discipline the function digest follows it — one digest
discipline, never two.

**49. Nondeterminism host seam (C50).** Decided: accept, with the propose-first step discharged
by fixing the trait shape: marrow-run gains one trait, `Nondeterminism`, with exactly two
methods — `now_nanos(&self) -> i128` and `entropy_u128(&mut self) -> u128` — as the single
nondeterminism crossing point; production impl `SystemNondeterminism` delegates to SystemTime
byte-identically to today's `Host::with_system_clock` (including its error→0 posture) and reads
OS entropy from /dev/urandom under W7.2's unix-only platform statement and entropy-failure
line — no new dependency (no rand dependency exists in the workspace); a fixed deterministic
impl serves tests. Host's capture-once clock semantics are unchanged (the provider is consulted
once at run start); marrow-store and the Backend trait never read clock or entropy — W3.4's
StoreUid mint is the second crossing-point migration, with nondeterministic values minted above
the seam and passed down, which is what makes a future SimStore a pure backend swap;
backend-contract.md gains the one deterministic-sim growth-path paragraph (W3.9). Crate APIs are
unstable (G1-5), so widening the trait for a future std::random (C58, deferred) is an ordinary
additive change, not a contract event. The SimStore backend, fault schedules, and replay harness
are post-v0.1 (after W3.8 freezes the fixture corpus and W7.2 closes gate 35).

**50. StoreStamp + the equality-token clarification (C17).** Decided: accept — one typed fact
`StoreStamp { store_uid, catalog_epoch, commit_id }` with a single owner beside the existing
fence/stamp constructor (metadata_stamp/StampFacts in marrow-run), rendered in W6.6's pre-stable
envelope under the key `store_stamp` with field spellings `store_uid`/`catalog_epoch`/`commit_id`,
and the write-produced commit coordinate rendered as sibling key `committed` only when the run
committed a write — omitted, not null, for read-only runs. One clarification line enters
foundations/01's developer-visible-durable-clock law, anchored by content: an opaque token
admitting only equality comparison is not a developer-visible clock — it never orders, and only
the catalog epoch orders durable time. DataToken analysis-API exposure ships when marrow-lsp
first files the need (named trigger, no sitting); backup-manifest summary and data-stats JSON
stay out of scope; depends on C43's StoreUid (W3.4).

**51. Recovery-point gating for destructive applies (C33).** Decided: require by default — an
apply whose witness contains a Retire outcome requires `--backup <path>` (the backup completes
via atomic temp+rename and validates before any store mutation, then the apply proceeds in the
same invocation) or the explicit `--no-backup` opt-out recorded in the apply receipt/log;
refusal code `evolve.requires_backup`; CatalogOnly and Backfill untouched. The named
Transform-inclusion question is resolved now rather than left open: Transform (and Default)
applies are excluded, because per data-evolution.md a transform may not read the member it
replaces and backfill populates only records that lack the member — these writes fill absent
cells and never delete or overwrite populated ones, so the pre-apply data survives in the store.
The durable classification rule is recorded with the decision: the recovery-point gate binds to
witness outcomes that delete or overwrite populated stored cells — in v0.1 exactly Retire — and
any future evolution verb with that property joins the gated class by this rule, not by a fresh
argument.

**52. Innovation ADR-amendment batch (sixteen items).** Decided: approve all sixteen items as
one batch, no carve-outs: C44 dense-CommitId wording (storage-engine/01; code-true — commit_id =
prior + 1 minted only at the commit stamp, rollbacks consume nothing; conformance law rides W3.2
as stated); C54 data.code sentence (tooling/03); C57 one-data-model scoping (language/01 +
storage-engine/03); C39 store-key deferral scoping (language/04); C58 randomness-deferral
annotation (language/07) + per-call-unique effect-class placeholder (storage-engine/02); C45
no-sibling-codec constraint (storage-engine/05); C48 id-allocation-policy reserved field
(catalog-identity/02); C15 ephemeral-mount-is-not-a-restore paragraph (storage-engine/05); C22
derived-root watermark-validity sentence (model-ir/04 + the docs/future sketch fix); C23
bounded-cardinality reserved sentence (storage-engine/03); C24 constraints-triangle seam
sentence (constraints design note) + the review criterion written into W3.1/W3.6 lane text; C25
outbox-idiom paragraph (backend-contract.md); C31 law-15 measurement-not-explanation note
(foundations/01; citation matches current numbering); C38 identity-codec decode-surface note
reserving exactly the spelling `std::identity::components` (language/05); G3-2
encryption-as-backend-profile clause (storage-engine/01); C28 `doctor.*` finding-code-family
reservation (decision-record note; rows enter error-codes.md only when W6.13 ships). Routing
clarification, not a carve-out: W1.1 owns the marrow-decisions repo only, so the marrow-repo
items route by file owner — C25's backend-contract.md paragraph rides W2.5, C22's docs/future
sketch fix rides W1.2, and C24's review criterion lands in the W3.1/W3.6 lane prompts.

---

## Wave 3 — Store, evolution, and catalog machinery

The catalog/evolution series coordinates with the live engine-resident-catalog refactor — it is
the named owner of the #24/#27 subtractions. The remaining tail lane sequences after the catalog
series.

Wave gate: full gate; evolution/backup fixture families feed the gate-35 ledger.

---

## Wave 5 — Checker architecture series (the hotspot; strictly sequenced, one lane at a time)

Runs after the language batch so the refactor churns once over the final v0.1 surface.

**W5.6 → Analysis-API innovation surface (C01, C02, C03, C08, C29, C30, G2-4).** After W5.5,
before the wave gate. Owns: `sites_for(catalog_id)` + the UseSite table from one post-lowering
walk (C01); the reserved StoreIndexId usage-bitmap field shape in CheckedFacts (C02,
unpopulated); the cost-fact audit against model-ir/01's mandate + one cost-shape query + one
known-cost conformance fixture (C03); the RenameAction type (source edits + synthesized
evolve-rename fragment through the canonical formatter) + one SavedDataBacked test (C08);
`evaluate_checked_read_only_expression` (synthetic-expression injection into a CheckedProgram
context; writes/host-effects/unindexed lookups rejected with source-level codes) (C29);
`evolution_preview(snapshot, backup: Option<&Path>) -> WitnessFactSet` (schema-only without a
data source; counts/samples from a backup path; live-store path explicitly deferred) (C30); one
oracle law that `marrow check` reads each source file exactly once and opens the store zero
times (G2-4, post-W5.4). Seed: each query's failing API test. Review: every new surface is
read-only, recomputes internally, and enters the recorded contract; no second semantic
classifier introduced (the eval path reuses the production checker). Done: the analysis-API
contract recorded at the wave gate includes all of the above; marrow-lsp builds green against
head.

**W5.7 → Checker field resolution.** Carries: static `check.unknown_field` for dotted field
access on resolvable base types. Owns: infer.rs field/optional-field resolution, diagnostics.rs,
docs/error-codes.md, and fixtures for local resource and saved-path unknown fields. Seed:
`b.typoField` where `b: Book` and `^things(id).nosuchfield` currently fail later or noisily.
Review: no did-you-mean search, no field diagnostic on Unknown bases, no runtime
`write.unknown_field` removal, and no grammar/runtime/evolution change. Done: schema-known field
typos are check-time diagnostics and `??`/unknown-flow secondary noise is suppressed.

**W5.8 → Required-field definite assignment.** Carries: the straight-line required-field
absence check for whole-root writes whose RHS is a var-typed local resource with never-assigned
required fields. Owns: a narrow marrow-check pass or rule, diagnostics.rs,
docs/error-codes.md, docs/language/types.md, and the required fixture. Seed: `var b: Book`
sets only a sparse field, then writes `^books(id) = b` where Book has a required field. Review:
constructor RHS and prior whole-record reads do not false-positive; branch/loop/early-return and
cross-function paths remain inconclusive runtime checks; W4.12 keyed-layer entries stay out of
scope. Done: the straight-line never-assigned case is caught statically and
`write.required_absent` remains the runtime catch-all.

**W5.9 → Unknown-flow diagnostic cascade suppression.** Carries: the retained half of
partial-check-data-tools: suppress `check.unknown_type` / `check.untyped_value` cascades from
already-diagnosed type-resolution failures. Owns: checker diagnostic emission over Unknown
propagation in crates/marrow-check/src/checks and focused fixtures. Seed: an annotation-site
`check.unknown_type` currently causes downstream Unknown-flow diagnostics naming no new problem.
Review: no partial-check mode, no data-command execution on failed checks, no relaxation of the
data-tools.md clean-check policy, and no new error code. Done: Unknown propagation behaves like
the Invalid marker precedent: the primary diagnostic remains, secondary cascades disappear.

Wave gate: full gate; analysis API contract recorded for marrow-lsp; the tooling/05 ABI-identity
revision text drafted at this recording per gate 48's fixed definition — a pure byte-stable
function of checked facts following the W5.4 discharge-digest pattern, reviewed in-lane, no user
sitting.

---

## Wave 6 — Operator surface and integration (CLI; concurrent by command)

Listing order is execution order. docs/cli.md integrates in lane order: W6.4 → W6.5 → W6.3 →
W6.6.

**W6.1 ∥ Format matrix + error-code trims (gate 24: removal #14 A+C, removal #15, III.C1).**
Owns: trace.rs Json/Jsonl deletion (text-only trace; `--trace` composes only with text format),
`jsonl` removed from `run` entirely (`run --format text|json` stays valid for `--dry-run`, and
`run --format json` becomes the gate-27 envelope switch once W6.6 lands), backup/restore
`--format` removal, `load_checked_project_with_format` routing for every surviving flag incl.
failure paths, cmd_run.rs hard-coded Text sweep, the three-way drift merge — `evolve.drift`,
`evolve.store_commit_drift`, and `evolve.plan_mismatch` collapse into one `evolve.drift` whose
`data.drift_kind` is `witness`, `store_commit` (payload `pinned`/`found`), or `plan_mismatch`
(payload `expected`/`staged`) — capability-kind row deletion, the C54 data.code emit path on
run.uncaught_error + its CLI assertion test (W6.6 renders it in the envelope), per-command
broken-marrow.json JSON-envelope tests in one new test file this lane owns. Seed: failing test —
broken config under `--format json` yields a parseable envelope. Review: tooling/03
branch-on-code+data preserved through the drift merge; no unbounded trace buffering remains.
Done: every emitted line honors the surviving flags.

**W6.2 ∥ CI-grade test + discovery (III.C2 a-b + gates 24, 31).** Owns: cmd_test JSON/JSONL
results — one pre-stable record per test ({name, outcome, code?, file?, span?}) plus a summary
record — `--filter <substring>` over qualified test names (zero matches fail closed as
`test.none` naming the filter; the summary reports selected/total), plain-path tests config +
fail-closed metacharacter rejection (any entry containing one of `*`, `?`, `[`, `]`, `{`, `}` is
`config.invalid`), discovery.rs test rewrites — `tests/*.mw` asserts `config.invalid`; bare
`tests` asserts the recursive walk — project-config.md, symlink rule documented. Seed: failing
test — CI parses one JSON record per test. Review: fail-closed everywhere a typo could silently
drop coverage. Done: glob pseudo-grammar absent; test_pattern_base deleted.

**W6.4 ∥ Single-file check deletion (removal #16; supersedes III.C4's cli.md item).** Owns:
cmd_check.rs ScratchDir/relocation deletion, check_cli.rs migration to project fixtures, cli.md
rewrite, usage error pointing at marrow.json. Seed: `marrow check file.mw` is a typed usage
error. Review: no scratch-project machinery survives anywhere. Done: one checking mode.

**W6.5 → (after W6.4 — shared cmd_check.rs) check --data retirement (gate 25, removal #18).**
Owns: cmd_check --data path deletion — an ordinary unknown-flag usage error with no alias,
fallback, or deprecation shim; the {kind: data_check, status: ok, records_scanned} success
envelope retired with the spelling (evolve preview grows no abbreviated mode) — the
`run.store_unstamped` help text + its error-codes.md row + cli.md + data-evolution.md +
data_orphan_cli.rs atomically; model-ir/02 wording landed W1.1. Seed: `evolve preview` asserted
as the one spelling in the migrated tests. Review: the pipeline stage survives untouched; only
the spelling dies. Done: absence scan for check --data across docs and help.

**W6.3 ∥ (after W6.1 — main.rs dispatch/HELP one-line arm, mechanical rebase) marrow init
(III.C3, gate 30).** Owns: the new command (~200 lines, pure file writer), quickstart-mirroring
scaffold incl. plain-path tests entry (`"tests": ["tests"]`) and the explicit
`"store": {"backend": "native", "dataDir": ".marrow/data"}` block, the matching quickstart.md
edit (lines 25 and 36) in the same lockstep change, cli.md section, quickstart leads with init.
Seed: failing CLI test — init → check → run → test all green through the production pipeline.
Review: scaffold text and quickstart are one fixture (drift-pinned); init rejects a target
directory whose name is not a valid Marrow module identifier (exit 1, config-family code)
instead of scaffolding an uncompilable defaultEntry. Done: nothing-to-running in one command.

**W6.7 ∥ (after W6.2 — shared marrow-project lib.rs; W6.2 owns project-config.md) acceptedCatalog
key deletion + required store block (removal #41; gates 32, 42).**
Owns: marrow-project config field → constant, validation arms — incl. the gate-42 (C56)
store-required arm: config.invalid on an absent store block, a two-choice diagnostic naming the
exact config spellings `"native"` and `"memory"` and showing the one-line memory form
(`"store": { "backend": "memory" }`) — three config tests + the store-required tests; doc pages
project-config.md and quickstart.md (the quickstart edit integrates after W6.3's scaffold
lockstep). Coordinated with the catalog refactor (W3.5). Seed: unknown-key error lists the
shrunken set. Review: validation fails closed; the diagnostic names both backends with the
exact config spellings; the gate-30 scaffold is untouched. Done: one
catalog path; no silent selection of the non-durable mode.

**W6.8 ∥ Operations page + truth-sweep remainder (III.B1 + III.C4 residue).** Owns:
docs/operations.md (single-writer model, deploy choreography for the one-epoch window, store
growth + backup-restore repack, full-scan notes, a Security section — at-rest delegation, 0600
modes, the encryption-as-profile pointer (G3-2), the no-captivity sentence — the backup format
is the named canonical exit (C51), the branch-workflow subsection — backup→restore→work→discard,
the commit-epoch lineage interpretation, diff/merge named as deferred (C14), one README sentence
— docs-tone locked),
project-field rendering pick (canonical absolute path) code+docs. Serve items moot. Seed: the
page's claims each traced to a test or ADR. Review: neutral
reference tone; every limit paired with its named growth-path ADR. Done: the persona's first
operational questions all have documented answers.

**W6.9 → Worked unhappy-path guide (III.B5; after W2.6, W3.7, and W6.5 — shared
data-evolution.md).** Owns: data-evolution.md
"Maintenance Migration And Repair, Worked" — leaf retype, store re-key (via the identity
constructor for non-int keys), data.orphan repair bracketed by integrity; each example mirrored
as a fixtures/v01/evolution fixture with a byte-identity tidy check; one sentence recording the
gate-51 classification rule — the recovery-point gate binds to witness outcomes that delete or
overwrite populated stored cells, in v0.1 exactly Retire; the "Branch and team
workflow" doctrine subsection — merged source is truth, dev stores are disposable, production
ids flow only through deployed catalogs and evolve apply — with the end-to-end merge-conflict
fixture: conflict markers → catalog.merge_conflict → resolve → check green → losing-branch
store hits the existing activation fence (C32). Seed: the fixtures
themselves, run through the production evolution harness. Review: any ergonomic gap surfaced is
filed as a finding, never patched around in the doc. Done: the escape hatch is paved and cannot
rot.

**W6.10 → Invocation spine extraction (G1-1; v01-full).** After W3.5 and W6.1 (the cmd_run.rs
Text sweep lands before the spine extraction); **strictly before W6.6.**
Owns: a new module inside marrow-run exposing ProjectSession::open(root, mode) and
session.invoke(entry), absorbing the cmd_run.rs check→identity-freeze→open→fence→auto-apply→
unstamped-refusal choreography; the admission token is the existing
(source_digest, accepted_epoch, engine_profile) triple fence_run already assembles (no second
versioning concept); marrow run and marrow test become thin renderers; the marrow-run crate-root
doc rewrite. No new crate, no new
public promises (API unstable per G1-5). Seed: one new test — an in-process open of a drifted
store returns the same typed fence refusal code the CLI renders. Review: CLI behavior
byte-identical over the existing run/test suites; no path opens a store without crossing the
admission module. Done: one invoke function both transports call; W6.6 inherits a thin
cmd_run.rs.

**W6.6 → Dry-run stability then entry invocation (III.B4 then III.E1; both own cmd_run.rs —
sequenced; after W6.10's spine extraction).** III.B4: dry-run classifies via check, skips
identity freeze and auto-apply, reports would-freeze/would-apply — report content, not errors: a
successful preview exits 0, and when the fence would not pass, dry-run reports and does not
execute — pinned by the
check_read_only_cli pattern (durable-identity stability, stated limits), plus the write-counts
accumulator in the dry-run report — per-root/per-index creates/writes/deletes, catalog-id-keyed
sink over the WritePlan executor, no-op outside `--dry-run` (C49). III.E1: `--arg name=value`
decoding against checked entry signatures per the gate-27 value grammar — per-scalar textual
forms from the language literal grammars; string is the raw remainder after the first `=`;
sequence parameters (element type scalar or enum only) repeat the flag in argv order with `[]`
as the empty-sequence spelling; identity params via the W2.6 constructor shape and key-component
guards (no second key-decoding owner), single-component keys only, composite-key identity
parameters excluded with a typed error naming the wrapper-entry pattern; resource-shaped
parameters rejected typed; the rejected `--args-json` alternative never ships — the two
error-codes.md rows `run.entry_surface` and `run.entry_argument` (kind runtime, exit 1, rendered
in the envelope), the pre-stable JSON result envelope (identity returns rendered in the marrow
data JSON identity form; resource-shaped returns excluded), per-invocation cost documented; the
`--arg` hostile-input fixture family (scalar edges) joins W2.2's CI family. Batched inside the
III.E1 half (the field reservations are one
change): data.code on run.uncaught_error rendered in the envelope (W6.1 owns the emit path)
(C54); the `store_stamp` envelope object with field spellings
`store_uid`/`catalog_epoch`/`commit_id`, plus the sibling key `committed` present only when the
run committed a write — omitted, not null, for read-only runs (gate 50, C17; StoreUid from
W3.4); the reserved nullable `signature_digest` field, null throughout v0.1 (gate 48, C10); the
reserved null raises field (C07; the stability.md note lands in W7.2); the argv/stdout transport
sentence in cli.md and the lane doc
(C47); the G1-2 implementation constraint — all signature-driven decoding lives in
marrow-run::entry (CheckedEntryCall is the seam), CLI parser and JSON envelope are thin
adapters, one in-process resource-parameter-rejection test as a review requirement;
Host::with_output_sink as an additive capability — the lane chooses sink vs buffer for envelope
capture in the same change as the gate-13 print contract; RunOutput.output is not deleted
(G1-3); one egress-regime sentence in the envelope spec (G3-3). Seeds: dry-run
catalog-byte-identity test; an end-to-end `--arg` fixture incl. an `Id(^store)` parameter.
Review: no back-door JSON object mapping (resource returns excluded); textual scalar forms
specified in cli.md in lockstep with modules-functions.md. Done: an external app can drive a
Marrow store with typed arguments.

**W6.11 → Evolve operator surface (C26 + C33; gate 51; v01-full).** After W3.5 (apply.rs
reshaped); sequenced with W6.5 if files overlap. Owns: cmd_evolve render.rs --scaffold
(per-obligation ready-to-paste evolve blocks through the production formatter; retire +
--approve-retire comment; default/transform skeletons with parseable placeholder holes;
blocking-diagnostic footer pointing at --scaffold) and the recovery-point gate (--backup /
--no-backup on applies whose witness contains a Retire outcome; backup completes and validates
before any store mutation; `evolve.requires_backup` code + its error-codes.md row, single-row
convention; the --no-backup opt-out recorded in the apply receipt/log). Seeds: a blocked preview
emits a scaffold that parses by construction; a Transform-only (or Default-only) apply proceeds
without `--backup` while a Retire-bearing apply without it exits `evolve.requires_backup` with
the store unchanged and the opt-out recorded in the apply receipt/log. Review: soundness lens
verifies backup-before-mutation ordering; no source editing or AST write-back in --scaffold; the
Transform/Default exclusion and the populated-cell-deletion classification rule are asserted.
Done: the compiler writes the migration; the destructive edge is fail-closed.

**W6.12 → Backup as read target (C15).** After W3.8 (formats settled) and W6.11 (shared
cmd_evolve module). Owns: the backend-source
extension point in the data-loader opening chain; `--backup <artifact>` on marrow data
dump/get/roots/stats/integrity (ephemeral MemStore mount via the existing restore-validation
path — read-only, non-activating, codec-version-checked, no lock); `evolve preview
--from-backup` (witness re-derived against the past epoch; drifted state = typed refusal); the
storage-engine/05 amendment text landed via gate 52. Explicitly excluded: run --at, test --seed,
writable rehearsal, emit-backup hook (all deferred). Seed: failing CLI test — dump --backup over
an artifact matches dump over the restored store, with the live store held open by another
process. Review: no durable write on any mount path; gate-7 lock contract untouched (no file
lock taken). Done: point-in-time inspection and preview rehearsal work without restore
choreography.

**W6.13 ∥ marrow doctor (C28; slippable).** After W2.5 + W3.7 + W6.3 (main.rs dispatch/HELP
one-line arm, mechanical rebase). Owns: one cmd module aggregating
existing typed facts — config parse, check summary, catalog digest validation, store-open probe
(store.locked with remedy), fence/stamp classification, engine-profile tuple, bounded integrity
sample capped at a named constant — one finding per line, stable code (the W1.1-reserved set),
exact next command. No repair actions, no full scans, no new error families. Seed: a failing CLI
test over a project with a held store and a drifted catalog producing both findings. Review:
every probe path read-only; finding codes match the W1.1 reservations exactly. Done: pure
wiring-and-rendering; may slip post-v0.1 without contract cost because the codes are reserved.

**W6.14 ∥ Typed remedy payload: index synthesis (C09; v01-full).** After the Wave 5 series
(checker quiet). Owns: one DiagnosticPayload variant (SuggestedIndex carrying the synthesized
admitting declaration), populated at the hidden-scan rejection site via with_payload(); one CLI
render line ("add: index byShelf(shelf, id)"); fixtures. The ??/exists and rename producers and
the marrow-lsp code action are deferred. Seed: failing fixture — the no-matching-index rejection
carries the exact declaration for a two-field lookup. Review: fires only at explicit rejection
(law 15 intact); the synthesized declaration always compiles when pasted. Done: the language's
defining error carries its machine-applicable fix.

**W6.15 → Stdout flush on fault rider.** Carries: W6.6's Host::with_output_sink path must emit
print output produced before a runtime fault; if W6.6 is not imminent, a minimal stopgap prints
and flushes the accumulated buffer in the text Err arm before fault reporting. Owns: cmd_run.rs
fault rendering and the W6.6 output-sink seam. Seed: a text-mode run that prints, then faults,
currently loses stdout. Review: stdout/stderr separation stays intact, log remains stderr, the
success-path buffering contract does not change, and JSON envelope handling waits for W6.6.
Done: text-mode users see prior print output before the fault report without a new output mode.

**W6.16 → Records vocabulary.** Carries: rename `records` to `cells` in `marrow data stats`
text/JSON and `data dump` summary JSON after W6.5 retires `check --data`. Owns:
data stats/dump renderers, data_cli_inventory.rs and data dump/paged render-contract tests,
docs/data-tools.md, docs/cli.md, and docs/quickstart.md. Seed: pinned output fixtures still
expect `records` for data-cell counts. Review: no per-root breakdown, no fence state or
StoreStamp in stats output, and no rename of `evolve preview`'s entity-level `records_scanned`.
Done: docs define one cell as one stored path/value pair and distinguish it from one entity as
one identity tuple.

**W6.17 → Value echo diagnostics.** Carries: bounded offending-value prose at three verified
runtime sites: conversion errors in stdlib/conversion.rs, unique-index conflicts in
index_maintenance.rs, and parseDuration/parseDate in std_pure.rs. Owns: those message
construction sites and focused CLI/runtime fixtures. Seed: each site currently reports the
failure without the rejected value or key. Review: prose-only message changes; no RuntimeError
struct, envelope, data-payload, sensitive-taint, saved-read absent, or new egress-surface change.
Done: messages include bounded scalar/key previews while structured payload work remains W6.6.

**W6.18 → Test observability.** Carries: two independent test-surface improvements: (A)
`std::assert::equal(actual: T, expected: T)` for scalar types with "expected X, got Y"
rendering; (B) a nullable `output` field in per-test JSON/JSONL records, coordinated with or
folded into W6.2 and reusing the W6.6 output-sink seam. Owns: assert stdlib descriptors and
runtime assertion code for A, and cmd_test JSON/JSONL record rendering for B. Seed: a scalar
equality failure that currently needs manual boolean assertions, and a failing JSON test record
that lacks captured print output. Review: no equality over resources/sequences/trees, no syntax
change, no isTrue operand recovery, no text-mode output surfacing, no passing-test output
capture, and no trace interaction change. Done: scalar assertion failures render both values and
machine-readable failed test records can carry output when JSON/JSONL is requested.

Wave gate: full gate; CLI surface frozen for the release contract.

---

## Wave 7 — Release proof

**W7.1 R11 test re-architecture (removals "Collapse the 500-fold inline schema duplication" and
"Fold 176 integration-test binaries"; the latter P3 — may slip without contract cost).** Owns:
the corpus constant in marrow-check test support (~60 verbatim cases; split-form after W1.7),
fixtures/v01 confirmation, migration of remaining runtime value suites and CLI semantic
re-tests into the fixtures/v01 conformance corpus with same-commit deletion of the Rust
originals (behavior inventory per suite; surviving crates/marrow tests are boundary-only),
then one tests/main.rs per crate (dead_code allowances removed, support compiled once;
docs/freeze-gate-evidence.md verification commands updated in the same commits). Seed:
suite-shape tidy checks. Review: golden/semantic coverage does not drop; behavior inventories
complete; golden updates reviewed as contract changes. Done: suite shape matches
docs/testing-architecture.md.

**W7.2 → Release contract (III.B2; gates 33, 35, 37, 45).** Owns: `--version` engine-profile
tuple,
docs/stability.md (stable: dotted codes, exit codes, backup format, marrow.json, catalog schema
as versioned ABI; not stable: store bytes across recompiles, message prose; the G1-5 Rust-API
row — all six workspace crate APIs unstable, marrow-lsp a coordinated consumer, the future
embedding contract one facade surface, never the internal crates; the G1-6 profile-ordering
sentence — the gate-37 decided sentence, verbatim, in the not-yet-stable/future-surfaces
section; the two G2-6 law-backed sentences — one fsync per transaction per W2.2,
constant-memory evolution staging per W3.4, each landing only after its lane passes review; the
C45 cell-codec single-interchange sentence; the C48 reserved-field reference; the C51 forward
reference to the gated JSONL export; the gate-45 `parent_snapshot_digest` field marked
reserved/semantics-undefined; the C07 raises-field
note — the G3-3 egress-regime table lands in W6.8's Security section, its one home), platform
statement
(unix-only — Linux and macOS; the entropy panic stays as the documented unix-only constraint, no
diagnostic plumbing), distribution statement (tagged v0.1.0 source release +
`cargo install --locked --path crates/marrow`; binaries and crates.io publication named as the
post-v0.1 fast-follow),
**storage-engine/04 flipped to Accepted on the assembled evidence packet** (the five families
and their producing lanes exactly per the gate-35 protocol) with LayoutEpoch 0
frozen — the flip executes per the gate-35 protocol, pre-authorized on the green five-family
ledger, no further sitting; any red family blocks the flip and the tag. Seed: a version-output
test. Review: every stability sentence traces to a closed lane's evidence; no
implementation-quality claim enters the contract. Done: the release is a defined promise.

**W7.3 ∥ Docs-truth + absence pass.** Owns: every docs/implementation page re-verified against
code; the cumulative absence/sibling scan for every removed pattern (serve, lsp, protocol.*,
at-sugar, `_` concat, write, finally, labels, out, inout, decimal ranges, quoted segments, map
sugar, check --data, single-file check, completion/, rebind/resume, touches_saved_data,
FutureEphemeralRootEffects, Match fields, savepoint journal, meta cells 01-03, evidence fields,
acceptedCatalog, tempfile, glob grammar, drift codes, capability kind, fictional helpers) run as
one recorded script; the two persistent CI tidy checks — exact compiled-dependency count from
cargo tree on the release target (changed only by an ADR commit) and the release binary-size
envelope (+25%, re-pinned per release as a within-release guard) — pinned in the same commit as
the evidence packet (G2-5); the README Scope-And-Security expansion — untrusted-input
enumeration, checksums-detect-accident-not-tampering, the capability table as the
determinism/embedding seam (G3-1). Seed: the recorded absence-scan script failing on one seeded stale spelling, then
clean. Review: scan patterns probed adversarially — each proven to match its pre-removal
spelling; implementation-page claims spot-checked against code. Done: the scan output is part
of the release evidence packet.

**W7.4 → Final gate + tag.** Fresh full workspace gate from clean worktrees; crash harness
green; storage-engine/04 Accepted with LayoutEpoch 0 frozen (gate 35); conformance suite (now
incl. reverse/bounded-scan and identity-component laws) green; docs corpus
deterministic; evidence packet assembled (base/head, changed files, gate output, reviewer
verdicts, absence scans; measured cold-start wall-clock, peak RSS, and binary bytes on a named
reference setup — recorded, never asserted (G2-6)); tag v0.1. After the tag is pushed, one
marrow-decisions commit deletes foundations/03 and its two README index entries (gate 36), with
the deletion commit hash recorded in the evidence packet.

---

## Seams

Every v01-foundation and post-v01-seam item, its owning lane, and the invariant the seam pins.
"Lane" names where the v0.1 deliverable lands; gate numbers refer to Wave 0 gates.

### v01-foundation (correct before surfaces freeze)

| Id | v0.1 seam | Owning lane | Invariant pinned |
|----|-----------|-------------|------------------|
| C36 | maybe ReturnType marker general from the start | W4.5 (gate 39) | one fact-model owner for maybe-returns; no stdlib-only special case to migrate later |
| C44 | dense-CommitId sentence + density conformance law | W1.1 + W3.2 | the committed CommitId sequence is gapless — the cursor algebra of replication and incremental backup |
| C54 | data.code on run.uncaught_error + tooling/03 fix | W1.1 + W6.1 (emit) + W6.6 (envelope render) | clients branch on code+data, never message text, on the top programmatic failure path |
| C56 | store block required in marrow.json | gate 42 + W6.7 + W1.1 | no silent selection of the non-durable mode |
| C57 | one-data-model law scoped to shape + reads | W1.1 | no doc-vs-binary seam on the flagship symmetry claim |
| G2-1 | fsync/commit/bytes counters on the counting decorator | W2.8 | deterministic cost meters are pure functions of (program, data) |
| G3-2 | 0600 artifact modes + encryption-as-profile clause + ops Security section | W2.4 + W1.1 + W6.8 | at-rest delegation is explicit; encryption is forever a backend-profile concern, never cell-layer crypto |

### post-v01-seam (pin in v0.1; build later)

| Id | v0.1 seam | Owning lane | Invariant pinned |
|----|-----------|-------------|------------------|
| C01 | UseSite table + sites_for query | W5.6 | the use-site index is inside the recorded analysis-API contract |
| C02 | reserved StoreIndexId usage-bitmap field (unpopulated) | W5.6 | fact-table shape stable for dead-data analysis |
| C03 | cost-fact audit + cost-shape query + fixture | W5.6 | model-ir/01's cost facts are real and queryable before the contract records |
| C04 | oracle-spec extension (operation-shape fixtures; deduction cases named) | W2.8 | the counting decorator is the one cost-conformance oracle surface |
| C05 | sensitive/declassify reserved + sink set pinned | gate 43 + W4.6 | the modifier position and the sole declassify escape are claimed before grammar freezes |
| C06 | write_effects_reachable fact + open-mode contract | gate 47 + W5.5 | query/command classification has one transitive owner; first run is always write-capable |
| C07 | reserved null raises field in the envelope | W6.6 (field) + W7.2 (note) | the failure-surface slot cannot be claimed by anything else when the envelope freezes |
| C08 | RenameAction type + contract + test | W5.6 | the LSP never re-derives catalog paths or invents evolve synthesis |
| C10 | ABI-identity decision + reserved nullable digest field | gate 48 + W6.6 | gate 37's checkpoint inherits a decided function-identity model |
| C11 | WriteFallibilityFact derived in lowering (IR-only) | W5.2 | write-site fallibility classified once, at the place owner |
| C12 | state_digest manifest field | W3.4 | content-addressed states exist from the first frozen format |
| C13 | data-tools.md reframed to state-vs-state | W1.2 | the diff design is state-based before anyone builds store-vs-source |
| C14 | branch-workflow doc + commit-epoch lineage interpretation | W6.8 | restored stores have a named lineage anchor with zero new durable fields |
| C15 | --backup mounts + preview --from-backup + not-a-restore amendment | W6.12 + gate 52 | the loader chain has a backend-source extension point; mounts are non-activating and lock-free |
| C17 | StoreStamp envelope fields + the developer-visible-durable-clock law line | gate 50 + W6.6 + W1.1 (law line) | truthful version stamps are in the ABI from day one; the token is never a second clock |
| C18 | retain grammar position + spec vocabulary mapping | W4.6 | the post-name clause position is reserved; retention = destructive on declaration, maintenance per write |
| C20 | `journal` reserved + no-triggers doctrine | gate 44 + W4.6 | no program claims the keyword; the doctrine binds all future derivation designs |
| C21 | per-entry footprint JSON + change-signal sentence | W5.5 | hosts invalidate by static intersection; the changed-id fields are contract, not incident |
| C22 | watermark-equality validity sentence | W1.1 (model-ir/04) + W1.2 (sketch fix) | derived-root validity includes data-change invalidation, not just manifest match |
| C23 | bounded-cardinality reserved sentence | W1.1 batch | the index surface has a named additive extension point independent of II.C3 |
| C24 | constraints-triangle sentence + W3.x review criterion | W1.1 batch + W3.1/W3.6 | predicate constraints will route through DataProof/CatalogOnly; one obligation model forever |
| C25 | outbox-idiom paragraph in backend-contract.md | W2.5 | the manual pattern and the deferred engine outbox cannot be confused post-release |
| C28 | doctor finding codes reserved (build slippable) | W1.1 + W6.13 | the triage contract cannot collide with other lanes' code assignments |
| C29 | evaluate_checked_read_only_expression | W5.6 | one canonical checked-evaluation entry point for eval and editor hovers |
| C30 | evolution_preview signature (schema-only / backup-backed) | W5.6 | editors wire against a stable witness-fact contract; live-store path explicitly deferred |
| C31 | Profile arm placeholder + law-15 note | W2.8 + W1.1 | the profile surface is not foreclosed; measurement vs explanation is settled once |
| C34 | named entry types: validation + value semantics, evolution excluded | gate 41 + W4.12 | the layer-entry surface lands without freezing an evolution contract II.D2 has not earned |
| C38 | codec-as-decode-surface note + reserved accessor form | W1.1 batch | identity opacity holds; the future accessor form cannot be claimed by other syntax |
| C39 | language/04 deferral scoped to store declarations | W1.1 batch | the storage-engine/04 freeze does not close the identity-key door |
| C40 | deliberate-non-associativity sentence | W4.1 | `??` chaining stays a designed extension point, not a parser accident |
| C42 | checker-level docs corpus (already W1.6) | W1.6 | the patterns page can land post-W4.8 with fixtures that cannot rot |
| C43 | StoreUid mint + meta cell + manifest field + restore-mint rule | W3.4 | store instances are distinguishable before the format freezes |
| C47 | argv/stdout named as the v0.1 transport | W6.6 | the future socket transport is additive, never a protocol break |
| C48 | reserved id-allocation-policy catalog field | W1.1/W7.2 | a GUID policy lands as catalog evolution, not a schema break |
| C49 | write-counts sink in the dry-run report | W6.6 | speculative outcomes report in catalog-id space; before/after sampling deferred cleanly |
| C50 | clock/entropy host-trait point | gate 49 + W3.9 | deterministic simulation is a backend swap, not a runtime retrofit |
| C51 | no-captivity sentence + gated-export forward reference | W6.8 + W7.2 | the exit is a supported surface; the JSONL export stays gated on tooling/05 + W5.3 |
| C52 | reserved parent_snapshot_digest sentinel (fail-closed) | gate 45 + W3.4 (field) + W7.2 (stability note) | incremental chains never require a FORMAT_VERSION break |
| C53 | consumption-contract decision record | gate 46 + W1.1 (language/07 text) + W1.6 (reserved rows) | std::json implements against a frozen design with one validity owner |
| C58 | randomness annotation + effect-class placeholder | W1.1 batch | std::random lands later through the effect model, not around it |
| G1-2 | decode-in-library constraint + in-process test | W6.6 | one semantic decode path for shell and embedded callers |
| G1-3 | Host::with_output_sink (additive) | W6.6 family | output capture has a host-owned boundary before any embedding API exists |
| G1-4 | transitive effect closure + read-only conformance test | W5.5 | read-only admission is provable; hidden write acquisitions are flushed out |
| G1-6 | gate-37 profile ordering + stability sentence | gate 37 + W7.2 | the envelope never acquires a wire-transport bias; the SQLite-shaped option is first |
| G2-3 | catch-site Error construction + cost-model sentence | W2.3 | resolved absence allocates zero at the raise site — the one honest v0.1 allocation law |
| G2-4 | source-read-count oracle law | W5.6 | check never re-reads sources or opens the store |
| G2-6 | two law-backed stability sentences + three evidence fields | W7.2 + W7.4 | performance claims are oracle-backed; latency stays recorded evidence |
| G3-1 | README Scope-And-Security expansion | W7.3 | the threat model is enumerated where it already lives — no sediment page |
| G3-3 | egress-regime table + envelope regime sentence | W6.8 + W6.6 | every emitting surface has exactly one regime; new surfaces must classify at design time |
| G3-4 | Capability enum + requires_capability column | W4.5 | one typed capability owner before host modules multiply |

---

## Deferred (explicit)

- **II.E5 local-iteration spec paragraph** — P2 by ruling; one paragraph, un-deferred by any
  docs/future local-collections work starting.
- **II.E7 std::json + III.B6 data-ingress guide (C53)** — fast-follow scope amendment to
  language/07; the unknown-tree consumption contract is decided at gate 46; std::json implements
  against that frozen contract, the decoder reusing the evolution/integrity leaf-decode family
  (one validity owner); the ingress guide is sequenced strictly behind it (a delimited-text
  interim guide is the fallback evidence for raising it).
- **II.D2 evolution into keyed child layers** — fails closed today; W6.9's recipe is the v0.1
  mitigation; un-deferred by the first real keyed-layer retype report.
- **II.C3 counted indexes (C59)** — P2; needs its own storage-engine/03-adjacent decision,
  drafted around the derived-data contract (plan-maintained, DerivedRebuild-verdicted,
  backup-excluded, restore-rebuilt, cost in source); a `carrying` surface is admitted only after
  separate validation against the cost model; purely additive.
- **III.D3 lexer delimiter recovery** — P2 editor-quality; un-deferred by marrow-lsp field
  feedback; full error-node AST stays a later editor lane.
- **IV.D4 interpreter value-model staging (G2-3)** — P2; sequenced after W5.2 whenever
  scheduled; mechanical and semantics-preserving; the allocation-constant harness (loop step ≤ C,
  point read ≤ K, interned field names) is its TDD seed, pinned only after IV.D4 repairs the
  known-broken constants.
- **II.E8 declared history layers** — P3 design sketch, hard-gated on II.D2, kill condition
  attached (if pressure grows toward general triggers, drop it).
- **Quoted-segment Phase 2** — blocked on the maintenance-surface syntax decision (ADR
  language/03's gated home).
- **Multi-reader handle model (IV.D2 item 3)** — designed contract recorded in W3.2; P2 design /
  P3 implementation alongside the deferred app-server ADR.
- **tooling/05 export boundary** — gate 37 decided: Profile A (linked-Rust embedding) then
  Profile B (export fn + TypeScript); the post-v0.1 checkpoint schedules implementation only,
  ordering not reopened; III.E1 the named interim.
- **salsa-style memoization** — post-0.1 behind the W5.1 surface; needs its own dependency
  decision.
- **Dead-data warnings (C02)** — populate the reserved bitmap; check.unused_index at warning
  tier after fixture-corpus noise validation; unread-member facts behind the same validation.
- **Full cost-shape facts + cost-conformance harness (C03/C04)** — transitive composition, loop
  depth, LSP hover, the committed cost baseline, and the exact runtime-equals-facts equality;
  after the checker gains the cost-fact tables W5.6 audits.
- **Sensitive taint pass + declassify builtin (C05)** — additive checker lane over the gate-43
  reservation; warning tier first.
- **Read-only cmd_run open path (C06)** — after W6.6 quiets; the W5.5 fact is the seam.
- **Raises sets (C07)** — propose-first: fault-family taxonomy + conservative catch semantics
  before any computation.
- **Schema-aware rename in marrow-lsp (C08)** — sibling repo consumes RenameAction.
- **Remedy producers #2/#3 + LSP code action (C09)** — after marrow-lsp builds its code-action
  surface.
- **Function table + committed ABI artifact (C10)** — with tooling/05 at the gate-37
  checkpoint; the format freezes with the table, not before.
- **Write-fallibility consumption (C11)** — the write-site-resolution design question, mirroring
  presence; the W5.2 fact is the prerequisite.
- **marrow data digest (C12)** — un-deferred by the first time feature needing state equality.
- **marrow data diff (C13)** — post-LayoutEpoch-0 freeze; equal-epoch baseline, cross-epoch
  growth; one ordered-stream adapter over the backup reader + the merge-join.
- **Branch/merge CLI (C14)** — the time-family capstone; derives from C12 + C13; typed
  three-way merge applied as managed writes under maintenance.
- **run --at / test --seed / writable rehearsal / emit-backup hook (C15)** — over the W6.12
  backend-source abstraction; run --at requires fencing against the backup's epoch.
- **DataToken analysis-API exposure (C17)** — when marrow-lsp first requests data-view
  invalidation; long-poll/watch loops follow with no infrastructure change.
- **retain newest N implementation (C18)** — gated on II.D2 landing and II.E8 surviving its
  kill condition; time-based retention stays out (clock-in-plan).
- **journal changes (C20)** — after LayoutEpoch 0, II.D2, and II.E8 resolve; open design
  questions recorded in the doctrine page (journal key type, evolution governance of entries,
  whole-record-delete interaction, bounded-read cost contract).
- **Per-root watermark cells (C21)** — one extra meta write per touched root per commit, atomic
  with data; the stale-cache bug class structurally eliminated.
- **max N bounded-cardinality indexes (C23)** — own storage-engine/03 amendment; witness
  integration (DataProof/RepairRequired on tightening) is the real scope.
- **Outbox worked guide + fixture (C25)** — first post-tag docs batch; not in
  data-evolution.md.
- **Seed data as code (C27)** — tooling/04 amendment is the gate; rides after W6.2's contract
  settles; test.seed outcome row; golden backup-mounted seeds follow.
- **marrow eval CLI (C29)** — wires the W5.6 seam to a subcommand; **marrow console** stays
  behind IV.D4 and its own design decision (docs/future names the growth path).
- **run --profile cost receipts (C31)** — the first post-v0.1 accepted-tooling decision,
  referencing the W2.8 seam and the law-15 note.
- **Named-entry-type evolution (C34)** — with II.D2; the W4.12 fence is the seam.
- **std::identity::components accessor (C38)** — propose-first stdlib form over the reserved
  slot; never dotted field access.
- **Identity-typed store keys (C39)** — single vertical lane post-W4.7: language/04 amendment,
  allowlist lift, key-codec tuples, catalog store→store references (retire fails closed),
  ordering laws, evolution discharge via W3.6's owner.
- **`??` chaining (C40)** — propose-first language/02 amendment after W5.2 stabilizes
  check_coalesce.
- **docs/language/patterns.md (C42)** — strictly after W4.7 + W4.8 integrate; ten
  checker-level fixtures, one per pattern.
- **restored_from provenance (C43)** — with the tooling/02 era (sync/feeds/incremental backup).
- **SimStore + seeded replay harness (C50)** — after W3.8 freezes the fixture corpus and W7.2
  closes gate 35; bounded optional CI soak; the generative workload explorer follows.
- **Typed JSONL export (C51)** — gated on gate 37, W5.3 type-identity unification, and an
  explicit tooling/03 revisit promoting one canonical mode from debug/admin to stable.
- **Incremental backup lane (C52)** — fills the reserved
  parent_snapshot_digest; chain verification + retention story.
- **std::random (C58)** — own Wave-0-style decision slot for the per-call-unique effect class;
  capability-table row over the G3-4 seam.
- **RunOutput.output deletion (G1-3)** — once tooling/05 exists and the envelope transport is
  settled; the sink is the seam.
- **Read-only ProjectSession (G1-4)** — the v0.2 embedding boundary: stale-epoch refusal,
  typed boundary error on write-closure entries, over the W5.5 closure and the gate-7 contract.
- **Embedding facade crate + profile A (G1-5/G1-6)** — at the gate-37 checkpoint: one facade
  surface over session and envelope types; internal crates never stabilize.
- **Host capability migration + derived authority fact (G3-4)** — provider map keyed by
  Capability; per-entry minimum-authority fact through the W5.5 closure; envelope field;
  the honesty clause ("the CLI grants everything but Maintenance") documents it.
- **Egress-regime sentences for future surfaces (G3-3)** — written when each surface (export,
  diff, eval, console, change feed, backup mounts) enters design; classification is a
  design-review requirement.
- **Not in scope by locked decision (do not relitigate):** query language/planner, migration DSL,
  nullable types, generics/aliases, FK constraints/cascades, app server (tooling/02 Deferred),
  `marrow catalog` command, `marrow explain`.

---

## What v0.1 must prove

One sentence, end to end: *the compiler that checks your source also governs the data you already
saved — and you can trust it with that data.* Sharpened by the subtraction review: trust is
earned as much by what v0.1 refuses to carry as by what it builds. Concretely, five demonstrable
properties: a record written is a record that exists (node cells, total presence proofs, no
silent existence loss); a byte committed is a byte that survives a crash, with in-tree evidence
rather than dependency folklore (pinned durability, kill-tests, the frozen encoding); the cost
the source states is the cost the engine pays (seek-based navigation under a counting oracle);
every published contract — spec example, error code, ADR sentence, CLI flag — is true of the
shipped binary, because everything false, phantom, duplicated, or consumer-less was deleted
rather than documented around (no fictional helpers, no second LSP, no protocol family for a
server nobody calls, no evidence fields re-proving what atomic commit already guarantees); and
the operator's recovery path never destroys what it protects (atomic backup, a crash window that
no longer exists, a worked repair recipe that cannot rot). The novelty — witness-gated evolution,
structural absence, the invisible catalog — is already built and already excellent; v0.1's job is
to make the foundation's own claims checkable, so the first skeptical evaluator who probes the
gap between the documentation and the binary finds none.
