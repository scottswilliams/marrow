# Lane 9: Source-Native Evolution And Activation

Status: integrated foundation. Future changes in this area are regressions,
hardening work, online-activation follow-up, or explicit product-decision work;
this file is a historical contract reference.

## Contract

Lane 9 supplies one source-native proof-discharge pipeline for evolution and
activation:

- `marrow check`, data-attached check, `evolve preview`, `evolve apply`, and
  repair admission consume shared proof facts;
- source-native `rename`, `default`, `transform`, and `retire` intent authorizes
  schema/data changes;
- preview is read-only and produces an exact witness;
- apply consumes only the exact witness and aborts on source, catalog, snapshot,
  engine, affected-ID, count, or approval drift;
- durable identity is recorded by state-establishing flows, with no separate
  catalog command;
- stale writers fail closed on catalog epoch, engine profile, and schema digest;
- v0.1 compatibility is strict and exact; any accepted compatibility window is
  bounded by checked catalog/runtime facts.

## Rejection Ledger

Rejected as v0.1 product surfaces:

- migration scripts, migration DSLs, and hidden schema-history ledgers;
- source-diff identity inference or best-effort rename preservation;
- unchecked transform shims;
- apply paths that do not consume the exact preview witness;
- repair paths that bypass catalog, proof-ledger, engine, or data checks;
- old-schema adapter runtimes outside explicit, bounded, checked compatibility
  windows.

Every surviving reference to migration scripts, source diff, best-effort
renames, or transform shims must classify the term as rejected, future-only, or
part of the checked source-native workflow.

## Future Direction

Activation should remain job-shaped. V0.1 can execute an activation job
immediately in one exact transaction, but future large rebuilds, backfills, and
transforms should use the same witness-derived job model with bounded chunks,
verification, publish, and close phases.

Compatibility adapters are future-only unless generated from checked
source/evolution facts, named, visible in tooling, bounded to a finite window,
and deleted after the window closes. Key-shape, resource-shape, layout, and
engine changes that cannot be proven as ordinary backfill use future shadow
decant rather than raw store patching.

If this surface is reopened, review must attack witness drift, destructive
approval scope, stale engine metadata, repair bypasses, compatibility-window
expiry, and partial publish visibility.
