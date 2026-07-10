# Security Policy

## Supported Versions

Marrow is experimental and unreleased. Security fixes are handled on the main
development branch. There are no supported release branches, backport promises,
or patch-release schedules unless one is announced with a future release.

## Reporting A Vulnerability

Do not open a public GitHub issue or discussion for a suspected vulnerability.

Email reports to williamssscott@gmail.com. Include enough detail to reproduce or assess the issue:

- the affected Marrow version, commit, or branch;
- the `.mw` source, project files, store or backup inputs, commands, or host inputs involved;
- the expected behavior and the observed behavior;
- any crash output, diagnostics, logs, or proof-of-concept steps that are safe to share.

Useful reports include compiler, runtime, CLI, storage, backup/restore, and
other reachable boundary issues that could affect confidentiality, integrity,
availability, project isolation, or handling of untrusted inputs. The legacy
client/server stack remains in scope while it is reachable.

## Response Expectations

You should receive a human response after the report has been reviewed.
Follow-up may ask for a smaller reproduction or clarification. Fix timing
depends on severity, reproducibility, and the experimental state of the affected
code; no hard response or disclosure SLA is promised before a release policy is
published.
