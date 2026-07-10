# Data Evolution

> **Status: retired proposal.** This page is non-authoritative and may conflict
> with the current vision. Do not implement it without an accepted target
> contract; see [Retired design notes](README.md).

Future counterpart of [`../data-evolution.md`](../data-evolution.md).

Current evolution changes one accepted catalog to the next through checked
source, explicit `evolve` intent, preview witnesses, and `evolve apply`. Future
evolution features keep that source-native contract: saved data moves because a
checked project declares how it should move, not because a tool rewrites raw
store bytes.

## Multi-record transforms

v0.1 supports narrow per-record `evolve transform` bodies. The transform computes
one saved member from the same record's other still-decodable members.

Multi-record transforms are deferred. A future multi-record transform must
declare its read set, write set, target identity rules, ordering, conflict
behavior, and transaction boundary. It must also define how preview witnesses
bound the work and how apply proves the live store still matches the preview.

Examples in this family include splitting one record into many records, merging
several records into one record, or computing a target from more than one saved
identity.

## Advanced repair

Current repair remains explicit maintenance code over modeled paths. Evolution
preview reports obligations and blockers; it does not synthesize arbitrary data
repair.

A future repair surface must name the exact invalid state it can correct, the
source facts that authorize the correction, and the data it may write. It must
not bypass required-field checks, catalog identity, generated-index maintenance,
or backup/restore validation.

## Evolution and data movement

Future data movement features such as typed diff/load, restore merge, repair
restore, and cross-engine restore are owned by
[Data Tools](data-tools.md). When those features interact with evolution, the
shared boundary is accepted catalog identity: data movement must either preserve
the accepted catalog or advance it through the same preview/apply discipline as
ordinary source evolution.

## Cross-engine recompile

A cross-engine restore is a data-tooling recompile, not an evolution shortcut.
It may re-encode valid modeled data under a different backend profile, but it
does not change source shape or accepted saved-data identity by itself.
