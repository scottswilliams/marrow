# Admission and activation

A program image is produced without a store and then bound to one. Admission
reads the store and decides whether the image may bind; activation makes the
binding and any accepted data change in one commit.

## Today

`marrow run --store` compiles the project and compares the result with the
store's binding. An identical program opens the store. A code-only change
rebinds the store to the new code, and every stored value stays in place. A
change to the durable contract or to the exported interface is
`store.contract_changed`; the store is untouched and the prior program still
runs ([changing the program](../operations/README.md#changing-the-program)).
Accepting a changed contract, with stored data carried across, is future work
([status](../status.md#not-yet-available)).

## Phases

Compilation produces a reproducible image without opening a store. It checks
types and demand and verifies the image. It grants nothing.

Admission compares one verified image with a read-only snapshot of one store
and writes nothing. It reports one of three outcomes: the image is already
active; a witness describes a supported transition to it; or the image is
rejected. A report grants nothing.

Activation consumes one witness, confirms that the store head still matches the
state the witness names, and commits the data change, the accepted schema
state, and the store's active-image binding together. A receipt follows the
commit. A stale head, a used witness, or a witness for another transition fails
without writing.

A witness is used once, in the process that produced it. It is not copied,
stored for later, or moved to another process. If a commit's outcome is
unknown, the attempt ends; recovery reopens the store and finds either the
complete old state or the complete new state
([interrupted commits](../operations/README.md#interrupted-commits)). Rolling
code back is another activation, to the earlier image.

## Accepted transitions

Additive activation admits exactly four changes:

- code and identity-spelling changes that preserve semantic identity and
  representation;
- fresh sparse fields or groups added to an existing entry shape;
- enum members appended after every existing member, keeping each existing
  member and its order;
- fresh, never-reused root or branch placements carrying a wholly fresh finite
  graph, including indexes over a fresh empty root.

These changes add metadata and rebind; no stored value is rewritten. One
further bounded transition builds a single new index over a populated root; it
reads that root once and states the bound on that read. Every other change is
rejected without writing: an ambiguous identity, a change of representation or
key order, a removal, a reordered or reused member, or a rebinding onto a
populated path outside these rules. One classifier decides from the image and
the snapshot.

## Restore

Restore creates a fresh store identity and a fresh admission and binding after
full logical validation. It does not switch an existing store between two
authoritative heads. Finalization requires every restored required root to be
present and valid, binds the accepted head over that state, and neither
evaluates initializers nor changes application values.

## Developer view

An attach whose contract and binding are unchanged rebinds and runs. Any other
change is one explicit action that reviews, reports in source vocabulary
(places, presence, demand, stored work), takes acceptance, and activates
atomically. A metadata-only transition leaves every value in place; an index
build names the root it reads and the bound on that read. A developer types
one identifier by hand, the ceiling id accepted when an image is built
([`marrow image`](../tools/cli.md#marrow-image)). Witnesses and hashes never
appear.

## Evidence

Crash injection at every admission, activation commit, and receipt boundary
leaves either the complete old state or the complete new state recoverable. A
rejected change of any kind leaves the active store usable.
