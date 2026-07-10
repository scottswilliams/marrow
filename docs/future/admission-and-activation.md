# Admission and activation

This page is future direction. Current evolution preview/apply is narrower and
is coupled to the existing catalog and project-session architecture.

## Goal

Compilation establishes program meaning. Admission then compares a verified
program image with a pinned store snapshot without mutating either. It should
return one of three outcomes:

- the image is already active for that store;
- an exact transition is permitted and accompanied by a state-bound witness;
  or
- activation is rejected with typed reasons.

Activation consumes a still-valid witness and atomically changes populated
data, accepted schema state, and the store's active-image binding. A receipt is
created only after the transition commits.

## Constraints

- Checking, compilation, and admission are read-only.
- A witness is bound to the image, store identity, observed commit, accepted
  schema, prior active image, transition plan, and required operator decisions.
- A stale, replayed, or mismatched witness is rejected.
- Data transforms run with restricted authority and explicit resource limits.
- Destructive transitions require explicit decisions and a defined backup
  posture.
- Restore and physical recovery are followed by full validation and fresh
  admission before execution resumes.
- No image executes against a store whose active binding names another image.

Provisioning, logical import, transition planning, activation, inspection,
maintenance, and physical recovery must be separate services rather than modes
on one session object.
