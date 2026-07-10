# Served execution

This page is future direction. The current experimental HTTP server is legacy
and does not establish the target served profile.

## Goal

The served runtime should host the same verified image, durable model, ordinary
business functions, path kernel, admission rules, and transition semantics as
the embedded runtime. Authenticated principals and concurrent terminals add
deployment concerns, not a second application model.

Every served committed history must correspond to a permitted ordering in the
reference execution semantics. The runtime therefore needs explicit rules for:

- isolation and conflicts;
- retries and idempotency;
- cancellation and disconnects;
- host effects that cannot be replayed;
- authority and revocation races;
- active image/store-version mismatches; and
- readiness, draining, and failure visibility.

Transport adapters decode requests into typed values and semantic addresses,
invoke the ordinary callable boundary, and encode typed results. They do not
own language semantics or route-local authorization.

## Promotion test

The same longitudinal local application should become a service used from two
independently authenticated terminals. Its durable declarations and ordinary
business functions must not be rewritten merely to become served. If that
rewrite is necessary, the embedded-to-served continuity hypothesis has failed.

Replication, failover, consensus, rolling mixed-version deployment, and high
availability remain separate future work even after the first served profile.
