# Data coexistence

This page is future direction. Marrow does not currently ship a supported general
facility for exchanging durable data with external systems, beyond the trusted
bulk importer noted below as evidence. It records a constraint the language holds
today and the direction that constraint protects; it proposes no import format,
manifest field, or syntax.

## Goal

A Marrow application rarely starts on empty ground. Real adoption begins with data
that already exists in another system — an export, a prior tool, a spreadsheet, a
legacy store — and often continues alongside systems that keep their own copy.
Marrow's durable model should make bringing that data in, and living beside it,
ordinary rather than exceptional: a program should be able to populate its durable
places from a bounded external source, and later profiles should be able to
exchange durable data with systems outside the program, without a representation
choice that quietly forecloses either.

The load-bearing property is that this is **priced, not promised**. Coexistence is
a cost a maintained application pays deliberately — a bounded, kernel-mediated
operation with explicit failure and explicit authority — not a capability asserted
in the abstract. A design earns a coexistence claim by demonstrating the operation
end to end at real scale, not by leaving room for it.

## Constraint held today

No representation or durable-format decision may make bulk external ingestion
structurally impossible. This is a standing constraint on every durable-format,
identity, index, and storage choice: a design that would require rebuilding data
outside the language, or that admits no bounded path from an external corpus into
durable places, is rejected on that ground alone. The constraint binds format
work directly, so the ability to ingest is preserved by construction rather than
recovered later.

The constraint has a matching discipline: the answer to a specific ingestion need
is a bounded operation that already fits the model, not a new lifecycle feature
bolted on under deadline. Adding machinery mid-adoption to make an otherwise
impossible ingestion possible is itself out of bounds — it would mean the
representation had already failed the constraint.

Every path into durable data remains subject to the language's rules regardless of
where the bytes came from. External bytes are not durable Marrow data merely by
being well-formed: an imported value passes the same typed places, presence rules,
and path kernel as a value a program writes itself, and it is created under the
same authority the store admits. Raw-byte validity is never sufficient to claim
that stored data is valid Marrow data.

## Evidence target

The standing evidence for this direction is the dogfood port's import step: a real
personal-application data export, at real volume, populating a provisioned store
through a trusted bulk importer, with every entry created through the path kernel
and no raw storage key, engine handle, or transaction object exposed to any caller.
That importer is implemented as a closed lifecycle-maintenance operation; the port
exercises it as the concrete, measured demonstration that bulk external ingestion
is both structurally possible and correctly priced through the kernel. The import
step is where a representation regression against the constraint above would first
show up as a failure rather than as a silent narrowing.

## Deferrals

The following are not current and are not specified here:

- exchanging durable data outward — export, backup interchange, or handing data to
  another system in a negotiated shape;
- living continuously beside an external system of record, including incremental
  synchronization, reconciliation, or change capture;
- ingestion of shapes beyond a bounded flat external source, including nested or
  referential external structure;
- format discovery, schema negotiation, or mapping configuration between an
  external model and Marrow's typed places.

Each becomes current only when working code demonstrates it under the constraint
above, at which point its concise rule moves into the reference and this page is
reduced to what remains unimplemented.
