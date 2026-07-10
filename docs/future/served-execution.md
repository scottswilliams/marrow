# Served execution

This page is later future direction. The current experimental HTTP server is a
legacy transport and does not establish the target served profile.

## Goal

A served runtime should host the same verified image, durable declarations,
path kernel, binding/admission rules, and ordinary business functions as the
local runtime. Authenticated principals/clients and concurrent terminals add
deployment semantics; they do not introduce a CRUD service model or second
application language.

## Required additional semantics

- principal and client attestation separated from path authorization;
- credential verification bound to typed issuer/client, request-region, store,
  and rotation context under a trust anchor outside application durable state;
- monotonic rotation and revocation whose high-water state cannot be reduced by
  an application write or silently resurrected by restoring an old store;
- invocation policy and revocation races;
- isolation, conflicts, retries, and idempotency under concurrent execution;
- cancellation, disconnects, and non-retryable host effects;
- active image/store-generation fencing and draining; and
- bounded failure, audit, readiness, and recovery behavior.

Transport adapters decode typed values, invoke the exact exported-function
boundary, and encode typed results. They do not own source semantics, physical
keys, or route-local authorization.

## Promotion test

The same populated local acceptance application should be usable from two
independently authenticated terminals without rewriting its durable declarations
or ordinary business functions. If transport or concurrency requires such a
rewrite, the local-to-served continuity hypothesis has failed.

Replication, consensus, failover, rolling mixed-version deployment, broad
online evolution, and high availability remain separate work after the first
served profile.
