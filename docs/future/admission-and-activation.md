# Admission and activation

This page is future direction. Current evolution preview/apply is coupled to the
prototype catalog, project lock, and session architecture.

## Goal

Durable interpretation and executable authority need different identities.

- A data-contract identity describes semantic paths, durable representations,
  codecs, and closed structural/write laws.
- An exact executable binding additionally names the program image, dependency
  graph, artifact provenance, exports, host imports, verified effects, resource
  limits, and accepted authority for one store.

Admission should compare a verified candidate with one pinned store snapshot
without mutation. It can report an already active generation, a supported
binding-only or data-contract activation, or typed rejection. A report grants
nothing; activation consumes a state-bound witness and independent decisions.

## Constraints

- Compilation, checking, and admission are read-only.
- Every different image or dependency graph requires a new exact binding even
  when the durable contract is unchanged.
- Candidate-specific acceptance and a reusable maximum ceiling are independent.
- Active images, graphs, generated host artifacts, and binding records are
  immutable and pinned outside package-cache eviction.
- External deployment state and store metadata use a crash-recoverable staged
  protocol; unresolved state cannot attach.
- Code rollback is a new forward activation, not an epoch rewind.
- Restore creates a fresh store identity and fresh admission/binding after full
  logical validation.

The beta data-contract transition vocabulary should remain deliberately narrow:
initial contract, current contract, one explicit predecessor, and addition of
exactly one new sparse path only when that path has no stored values. A source
rename that preserves semantic identity is not a data-contract transition; it
is a contract-preserving binding-only activation. Arbitrary user transforms and
online mixed-version writers remain outside this vocabulary.

## Evidence target

Crash injection at every staging, store commit, receipt, and external publish
boundary must leave the old or new exact generation recoverable, never a mixture.
Unaccepted code, effect, host, provenance, or contract changes must leave the
active store usable.
