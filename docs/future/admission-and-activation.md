# Admission and activation

This page is future direction. Current evolution preview/apply is coupled to the
prototype catalog, project lock, and session architecture.

## Goal

Durable interpretation and executable authority need different identities.

- A data-contract identity describes semantic paths, durable representations,
  codecs, key order, and required-root/keyed-entry topology. Exact mutation,
  transaction, prune, page, and continuation behavior belongs to the language
  and validated executable profile rather than being hidden in this storage
  identity.
- An exact executable binding additionally names the program image, dependency
  graph, artifact provenance, exports, host imports, verified effects, resource
  limits, and accepted authority for one store.

Admission should compare a verified candidate with one pinned active-store or
exclusive restore-staging snapshot without mutation. It can report fresh
provision, an already active generation, a supported binding-only or
data-contract activation, restore readiness, or typed rejection. A report
grants nothing; activation consumes a state-bound witness and independent
decisions.

The owner-facing review should combine package, public API, host import, effect,
retained-graph, and store-specific consequences without turning source analysis
into executable authority.

## Constraints

- Compilation, checking, and admission are read-only.
- Every different image or dependency graph requires a new exact binding even
  when the durable contract is unchanged.
- Candidate-specific acceptance and a reusable maximum ceiling are independent
  checks. The beta may record candidate acceptance in one final executable-
  binding action without introducing a separately frozen acceptance identity.
- Active images, graphs, generated host artifacts, and binding records are
  immutable and pinned outside package-cache eviction.
- External deployment state and store metadata use a crash-recoverable staged
  protocol; unresolved state cannot attach.
- Code rollback is a new forward activation, not an epoch rewind.
- Restore creates a fresh store identity, deployment identity, and
  admission/binding after full logical validation. It does not atomically switch
  an existing deployment between two authoritative store heads.
- Restore finalization is distinct from fresh provisioning. It requires all
  restored required roots to be present and valid, binds the accepted head over
  that state, and neither evaluates initializers nor changes application values.
- Genesis and existing-generation admission produce their state-bound witnesses
  directly. Restore admission first produces pure reviewed facts that grant
  nothing; lifecycle joins them to its exclusive staged-state generation before
  constructing the distinct opaque restore witness. That witness also binds the
  canonical logical-state digest and complete required-root evidence. No witness
  kind can be substituted for another activation path.
- Restore staging leases and witnesses are nonserializable. A pre-head process
  failure cannot resume one; beta recovery reimports and revalidates into a new
  staged deployment before constructing another witness.

The beta retained-graph transition vocabulary should remain deliberately narrow:
fresh provision of required roots from image-verified bounded canonical initial
values, a binding-only change, and addition of exactly one new sparse path only
when its complete prefix has no stored values. A source rename that preserves
semantic identity and representation is a binding-only activation, not a
data-contract transition. Identity ambiguity, retained representation or key-
order changes, removal, and nonempty
rebinding are rejected without mutation. Arbitrary transforms and online
mixed-version writers remain outside this vocabulary.

Admission can compare identities, representations, key laws, and stored
presence. It cannot prove that a maintainer has preserved the human meaning of
unchanged bytes. Change review must state that limitation; an intentional new
meaning receives a fresh identity rather than being disguised as a rename.

## Evidence target

Crash injection at every staging, store commit, receipt, and external publish
boundary must leave the old or new exact generation recoverable, never a mixture.
Unaccepted code, effect, host, provenance, or contract changes must leave the
active store usable.
