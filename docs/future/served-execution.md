# Served execution

A served runtime hosts the same image, durable declarations, and business
functions as the local runtime, for several authenticated terminals at once.

## Today

Marrow has no served runtime. The only transport is the local supervised
channel between one supervisor and one runner
([TypeScript client](../tools/typescript-client.md)), and `marrow serve`
reports `cli.command_unsupported`. [Status](../status.md#trust-boundaries)
lists the trust boundaries of that profile.

## Direction

A principal's attestation is separate from path authorization. Credentials are
verified against a trust anchor kept outside application data. Rotation and
revocation state is monotonic: an application write cannot reduce it, and
restoring an old store cannot resurrect it. Revocation takes effect for
in-flight invocations.

Concurrent invocations need isolation, conflict handling, retries, and
idempotency. Cancellation, disconnects, and non-retryable host effects each
have a defined outcome. Running invocations drain before a new image is
activated. Failure, audit, readiness, and recovery are bounded.

A transport adapter decodes values, invokes an export, and encodes the result.
It owns no source semantics, physical key, or route-local authorization.

## Public paths

The compiler could project selected exports or addresses to stable typed URIs
and check key parsing, route collisions, wire shapes, and whether an invocation
grant is narrower than the export's demand. Publication stays distinct from
storage and authority:

- a private durable place is not automatically public;
- a source rename does not silently change a public path;
- exact read authority does not imply collection traversal;
- publishing a route grants no permission; and
- physical key bytes never become URLs.

An HTTP or routing library consumes that metadata. Public transport, error
confidentiality, timing, cache invalidation, and principal policy need their
own threat and operational models.

## Security obligations

A multi-user pilot requires tamper evidence and an audit trail for durable
data. A served deployment additionally requires encryption at rest and image
authenticity. The local single-owner profile does not exercise these
obligations: there is no second principal to attest to, no shared store to
audit against, and no untrusted medium between compilation and execution. An
obligation becomes current when code and tests enforce it and the reference
describes it.

## Promotion test

The same populated local acceptance application should be usable from two
independently authenticated terminals without rewriting its durable declarations
or ordinary business functions. If transport or concurrency requires such a
rewrite, the local-to-served continuity hypothesis has failed.

Replication, consensus, failover, rolling mixed-version deployment, broad
online evolution, and high availability are separate work after the first
served profile.
