# Data Evolution

Future counterpart of [`../data-evolution.md`](../data-evolution.md). Project
compilation over source, catalog, and live data — including preview/apply
discharge, typed transforms, and catalog-owned stable identity — ships today
and is documented there. This page records the designed extensions: online
activation jobs, compatibility windows, shadow decant, store recompilation,
and typed export/import artifacts.

## Online Activation Jobs

Future OLTP activation is a compiler-owned job, not a migration script. The
source, accepted catalog, checked facts, engine profile, and data snapshot define
the semantics. A job records execution evidence derived from an exact preview
witness.

The intended protocol is:

1. `preview` produces the exact witness.
2. `start` creates a durable job from that witness.
3. `backfill` processes bounded, deterministic chunks.
4. `verify` proves required fields, transforms, derived indexes, uniqueness, and
   shadow-layout identity facts.
5. `publish` advances the readable catalog epoch in a small commit.
6. `close` drains old runtime generations, removes adapters, and purges retired
   physical state.

Today's implementation collapses those steps into one exact local apply, but
the public facts must not assume a future online system can hold a global
write fence for the entire backfill. A transform that runs as an online job
adds cancellation and checkpoint behavior to today's exact apply.

## Compatibility Windows

Future server runtimes admit compiled programs by catalog epoch and runtime
generation. The default v0.1 policy remains exact epoch/schema equality. A future
online compatibility window is finite and normally spans one old epoch to one new
epoch.

Old reads may use compiler-generated typed adapters. Old writes are rejected
unless the compiler proves an adapter lowers them to the latest write plan and
maintains every active or building durable fact. Adapters are named,
digest-stamped, visible to tooling, and deleted when the window closes.

## Shadow Decant

Changing a store's identity key shape, reshaping a populated resource/layer, or
moving between layouts/engines may require a shadow-decant workflow instead of
an in-place backfill. Shadow decant writes a new store or layout in chunks,
bridges a bounded set of writes, verifies identity/count/checksum facts, publishes
a small binding change, and then closes the compatibility window. It is the
Marrow-native version of online copy/cutover, still governed by source and
catalog facts rather than raw store rewrites.

## Store Recompilation

Changing the storage engine is also compilation. Marrow should be able to read a
consistent typed snapshot from the old store target, validate it against source
and catalog, and write the same durable program data to the new target.

## Repair And Typed Artifacts

Future export/import artifacts carry enough catalog/source fingerprinting to
validate what their saved data means, and import compiles the artifact into the
target project/store.
