# Threat posture

This page records Marrow's current supply-chain floor and the deferred security
obligations that follow from the vision. It is not a claim of protection, a
security specification, or a statement that any listed obligation is met. Each
item is labelled by evidence level: **current** (present in the repository and
evidenced by code, tests, or CI), **future** (unimplemented direction with a
recorded trigger), or **deferred** (recorded so it is not lost, with no owner or
trigger yet).

## Current supply-chain floor

The following are present today and evidenced by continuous integration and the
contributor rules:

- A non-gating advisory CI job scans the committed `Cargo.lock` against the
  RustSec advisory database (`cargo audit`) and emits a CycloneDX software bill
  of materials (`cargo cyclonedx`) for the workspace. As of the job's
  introduction the audit reported no advisories across the workspace dependency
  set. The job is advisory: an upstream advisory or tooling change does not
  block integration and is triaged as a finding.
- The workspace carries zero `unsafe` code, enforced by the required CI matrix
  (`clippy … -F unsafe-code`).
- A new dependency requires explicit maintainer approval and a license review;
  repository source remains Apache-2.0. This bounds the trusted dependency set
  rather than protecting against a compromise of an admitted dependency.

These measures reduce exposure to known-vulnerable and unreviewed dependencies.
They do not establish confidentiality, integrity, or authenticity of stored
data or program images, and no measure on this page may be described as making
the system secure.

## Deferred obligations with recorded triggers

The following four obligations are unimplemented. Each carries a trigger — the
deployment profile at which it must be addressed before that profile ships.
Until its trigger, each remains future direction, not a gap in a claimed
guarantee, because the corresponding profile is itself future (see
[local applications](local-applications.md) and
[served execution](served-execution.md)).

- **Tamper-evidence** for durable data — future; required **before any
  multi-user pilot**.
- **Audit trail** of durable changes — future; required **before any multi-user
  pilot**.
- **Encryption at rest** for the durable store — future; required **before any
  served deployment**.
- **Image authenticity** for compiled program images — future; required
  **before any served deployment**.

The current local, single-operator profile does not exercise these obligations:
there is no second principal to attest to, no shared store to audit against, and
no untrusted medium between compilation and execution. The
[served execution](served-execution.md) page records the principal, attestation,
rotation, and audit semantics a served profile introduces; these four items are
the security-specific subset of that work and are surfaced here so they are not
lost at an epoch break.

## Further deferrals

The foundation review that identified the four obligations above enumerated
eight security items in total. The remaining four are recorded here as
**deferred**: they have no owner and no trigger in the current plan, and their
enumeration is preserved in the review record in Git history. They are surfaced
so that a later served or multi-user profile revisits the full set rather than
only the four items with recorded triggers. None is claimed to be addressed.

## Evidence needed to make any item current

An item moves off this page only when its behaviour travels the production path
and is owned by code, tests, and the canonical reference — for example, a
tamper-evidence or audit mechanism exercised through the durable runtime with
adversarial tests beside the invariant, or an image-authenticity check enforced
by the verifier. A CI advisory, a bill of materials, or a policy statement is
supply-chain hygiene; it is not evidence that any deferred obligation is met.
