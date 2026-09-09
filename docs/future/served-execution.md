# Served execution

A served runtime would expose one program and store to authenticated terminals.
It is deferred beyond the [local beta](../vision.md#beta-scope).

## Today

Marrow has no served runtime. The existing transport is a supervised local
channel ([TypeScript client](../tools/typescript-client.md)); `marrow serve`
reports `cli.command_unsupported`. [Status](../status.md#trust-boundaries)
records the local profile's trust assumptions.

## Direction

Retain one store owner and serial mutating invocations, with ownership from
before the first durable decision read through return. Lost replies never imply
rollback or authorize automatic replay. Transport adapters consume typed
exports and carry no language semantics or physical keys.

Authentication and path authorization are separate. Application data cannot
mint credentials or grants, and restoring old application data must not revive
revoked authority. Public routes project from semantic facts: publication
grants nothing, a private place is not automatically public, and physical keys
never become public addresses.

Reader overlap, cancellation, draining on activation and transport backpressure
need bounded lifetime and failure models before implementation. No snapshot
protocol, reservation scheduler or parallel runtime is selected in advance.

## Security obligations

Multi-user operation requires tamper evidence and an audit trail; serving also
requires image authenticity and encryption at rest. Credentials, revocation,
in-flight authority, error confidentiality and hostile-storage recovery need an
explicit threat model and independent evidence. The local beta does not satisfy
or cancel these obligations.

## Promotion test

The same populated local application should work from two independently
authenticated terminals without rewriting its durable declarations or business
functions. That continuity is a hypothesis, not a compatibility promise.

Qualification must include denied access, stale/revoked authority, restore,
lost replies and bounded resource behavior. Replication, multiple-store
coordination, failover, mixed-version deployment and general online evolution
remain separate future work.
