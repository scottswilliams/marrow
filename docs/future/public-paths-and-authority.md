# Public paths and authority

This page is future direction. Current bearer-authenticated serving is a legacy
transport and does not implement compiler-integrated authorization.

## Public paths

Durable state is private unless the program explicitly publishes an address
space or callable. A URI should decode to a typed semantic address before
application access. Publication defines an external representation; it neither
exposes physical storage nor grants authority.

The tree/URI relationship is structural:

```text
^patients(patient).visits(visit)
<-> /patients/{patient}/visits/{visit}
```

The mapping need not be one-to-one, and a source rename need not imply a public
URI change.

## Authority

The compiler can describe each callable's maximum durable-path effects. That
static bound is not sufficient permission. At runtime every concrete path access must
be allowed by both the callable's compiled effects and an invocation capability
constructed by a trusted host.

Capabilities should be typed regions of the semantic path graph. They can be
attenuated to a root, entry, or subtree, but application code cannot forge or
widen them. Local-owner, invocation, inspection, maintenance, activation, and
physical recovery authorities remain distinct.

Authentication adapters establish typed principal identity; they do not return
authorization verdicts. Generated clients and UIs are convenience projections,
never enforcement boundaries.

## Limits

Authority-safe errors, diagnostics, audit, change streams, timing, and other
observables require explicit threat analysis. Path checks alone do not prove
noninterference, confidentiality, regulatory compliance, or absence of side
channels.
