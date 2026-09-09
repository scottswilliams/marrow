# Admission and activation

Admission decides whether a verified program may use a store. Activation makes
an accepted program or data change atomic.

## Today

An identical program opens its store. A code-only change rebinds it and leaves
every value in place. A changed durable contract or exported interface is
refused without mutation; the prior program remains usable
([changing the program](../operations/README.md#changing-the-program)).

## Beta direction

Keep compilation, admission and activation separate. Compilation produces an
image without opening data. Read-only admission returns an already-active
verdict, an exact image-and-store-state witness for a supported transition, or a
rejection. It performs no mutation and grants no application authority.

Activation consumes the witness, checks its exact state, and commits data,
accepted schema state and the active-image binding together. A receipt follows
commit. Stale, mismatched and reused witnesses cannot authorize a write.
Body-only and binding-only changes follow the same ownership rule.

The minimum populated-store update adds a sparse field to an existing resource
and changes ordinary export code or its interface to use that field. Existing
values remain in place; the added field starts absent. Admission classifies the
interface and demand change separately from data compatibility. Any required
ceiling expansion needs explicit owner acceptance.

General graph evolution, required-field rewriting, enum changes, placement
changes, renames and an index build over populated data are deferred. An index
build enters beta scope only if the maintained update journey cannot avoid it;
its input and stored work then need explicit bounds. Every unsupported change
refuses without damaging the prior program or data.

Ordinary attachment stays implicit. A supported contract change is an explicit
review and activation in source vocabulary. Internal witness, identity and hash
bookkeeping is carried by tools, with no hand-edited metadata or fabricated
storeless invocation to prepare an update.

## Recovery and restore

An uncertain outcome remains uncertain until authoritative evidence resolves it;
reopening alone does not guarantee that it can be classified. No automatic
mutation replay is permitted.

Recovery must validate the image, schema and complete logical store, then obtain
a fresh read-only already-active admission verdict before service resumes.
Restore validates a complete logical backup into a fresh store identity and
binding. Neither process evaluates application initializers or silently changes
application values.

## Evidence

A populated application keeps all old values across the selected additive
update. Wrong identity, incompatible shape, insufficient authority and stale
witnesses leave the old application usable. Fault injection at publication,
activation, commit and receipt boundaries distinguishes complete old, complete
new and unresolved outcomes on each supported native platform.
