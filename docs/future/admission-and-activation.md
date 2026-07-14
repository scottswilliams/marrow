# Admission and activation

This page is future direction. Current evolution preview/apply is coupled to the
prototype catalog, project lock, and session architecture.

## Goal

Marrow separates producing an image from binding it to a store. Compilation is
storeless. A later lifecycle admits a verified image against one store and, on a
supported transition, activates it. The two lifecycle phases have different
authority: admission only reads, and only activation writes. Interpreting durable
state and holding executable authority are distinct identities that this lifecycle
keeps separate.

## Phases

Compilation should produce a reproducible program image without opening any
store. It resolves packages, checks types and effects, and verifies the image
independently. It reaches no store snapshot and grants no authority.

Admission should compare one verified candidate image against a read-only
snapshot of one store and perform no mutation. It reports exactly one outcome:
the candidate is already active for that store; an exact state-bound witness
describes a supported transition to it; or the candidate is rejected. A report
grants nothing.

Activation should consume one fresh state-bound witness, recheck that the store
head still matches the exact state the witness was bound to, and atomically
commit the durable data change, the accepted schema state, and the store's
active-image binding together. It issues a receipt only after the commit is
confirmed. A stale head, an already-consumed witness, or a witness offered for
another activation path fails without mutation.

## Two identities

- A data-contract identity describes semantic paths, durable representations, key
  order, and root and branch topology. Exact mutation, transaction, and traversal
  behavior belongs to the language and the validated executable profile rather
  than to this identity.
- An exact executable binding additionally names the program image, dependency
  graph, artifact provenance, exports, host imports, verified effects, resource
  limits, and accepted authority for one store.

Every different image or dependency graph requires a new exact binding even when
the durable contract is unchanged. A source rename that preserves semantic
identity and representation changes the binding, not the contract; an intentional
new meaning receives a fresh identity rather than being disguised as a rename.
Active images, dependency graphs, generated host artifacts, and binding records
are immutable and pinned outside package-cache eviction.

## Witnesses

A witness names one supported transition bound to the exact store state observed
during admission. Witnesses are neither copyable nor serializable, and are
consumed in the same process by the activation that produced their premise. A
witness cannot be reused, stored for later, moved to another process, or
substituted for a witness of another transition kind.

An activation whose commit outcome is indeterminate poisons and closes the
attempt rather than retrying. Recovery reopens the store and classifies the head
as either the complete old state or the complete new state; it never resumes or
replays the interrupted attempt. Code rollback is a new forward activation to the
earlier image, not an epoch rewind.

## Accepted transitions

The beta transition vocabulary is deliberately narrow. Metadata-only additive
activation admits exactly:

- code and stable-identity spelling changes that preserve semantic identity and
  representation;
- fresh optional sparse fields or groups added to an existing payload;
- append-only algebraic-type members that preserve every existing member code and
  its order; and
- fresh, never-reused root or branch placements carrying a wholly fresh finite
  graph, including indexes over a fresh empty root.

These changes add metadata and rebind without rewriting stored values. One
separately typed bounded add-index transition additionally builds a single new
compiler-maintained index over a populated existing root. Every other change is
rejected without mutation, including identity ambiguity, representation or
key-order changes, removal, member reordering or reuse, and any rebinding onto a
populated path that the additive rules do not cover.

One pure classifier decides these outcomes from the candidate image and the
read-only snapshot. The beta has no broad staging or evolution machinery, no
arbitrary data transforms, and no online mixed-version writers.

## Restore

Restore creates a fresh store identity and a fresh admission and binding after
full logical validation. It does not switch an existing store between two
authoritative heads. Restore finalization requires every restored required root to
be present and valid, binds the accepted head over that state, and neither
evaluates initializers nor changes application values.

## Developer surface

The machinery above is internal. The intended developer surface is: an attach
whose durable contract and binding facts are exactly unchanged rebinds
automatically through the same lifecycle actor and just runs; any other change
is one explicit action that reviews, renders a report in source vocabulary
(places, presence, effects, authority, stored work), takes acceptance, and
activates atomically. Witnesses, contract hashes, generations, and ceiling
identifiers never appear in ordinary guidance or require human transcription.

## Limits of admission

Admission can compare identities, representations, key laws, and stored presence.
It cannot prove that a maintainer preserved the human meaning of unchanged bytes.
Change review must state that limit.

## Evidence target

Crash injection at every admission, activation commit, receipt, and external
publish boundary must leave either the complete old generation or the complete new
generation recoverable, never a mixture. A rejected or unaccepted change — code,
effect, host, provenance, contract, or an unsupported transition — must leave the
active store usable.

This page states direction. [Durable programming](durable-programming.md)
describes the durable operations these transitions bind, and [project
status](../status.md) separates current behavior from future direction.
