# Data coexistence

Data enters a store from outside and leaves it whole. No representation
decision makes bulk ingestion impossible.

## Today

`marrow import` creates a store and fills it from a file of JSON objects, one
entry per line, committed in bounded batches
([`marrow import`](../tools/cli.md#marrow-import)). Each imported entry is
created through the same typed places and presence rules as a value the program
writes, under the authority the store admits. The importer holds no raw key or
engine handle. Current logical backup and fresh restore are described in the
[operations reference](../operations/README.md#logical-backup-and-fresh-restore).

## Direction

Every durable-format, identity, and index change must preserve the complete
logical backup and fresh-restore round trip. Current sparse additions preserve
stored values through [explicit apply](../tools/cli.md#marrow-apply). Broader
changed-contract data continuity belongs to [admission](admission-and-activation.md).

Every such decision also keeps a bounded path from an external corpus into
durable places. A design that requires rebuilding data outside the language is
rejected on that ground alone. Importing is a bounded operation with explicit
failure and explicit authority.

Four things are deferred. Handing data to another system in a negotiated
shape beyond backup. Continuous synchronization with an external system of
record. Ingestion of nested or referential external shapes. Format discovery
or mapping configuration between an external model and durable places.

## Evidence

An import larger than memory populates a provisioned store through bounded
batches, and a backup of that store restores into a fresh one with every entry
present.
