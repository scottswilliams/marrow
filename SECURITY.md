# Security policy

## Supported versions

Marrow is unreleased. Security fixes go to the main development branch. There
are no supported release branches, backport promises, or patch-release schedules
until a release announces them.

## Reporting a vulnerability

Do not open a public GitHub issue or discussion for a suspected vulnerability.

Email reports to williamssscott@gmail.com. Include enough detail to reproduce or
assess the issue:

- the affected Marrow version, commit, or branch;
- the `.mw` source, project files, store inputs, commands, or host inputs
  involved;
- the expected behavior and the observed behavior;
- any crash output, diagnostics, logs, or proof-of-concept steps that are safe
  to share.

Useful reports cover the compiler, runtime, CLI, editor server, store, and any
other reachable boundary where confidentiality, integrity, availability, project
isolation, or the handling of untrusted input could be affected. The current
trust boundaries are listed in [status](docs/status.md#trust-boundaries).

## Response expectations

You should receive a human response after the report has been reviewed.
Follow-up may ask for a smaller reproduction or clarification. Fix timing
depends on severity, reproducibility, and the state of the affected code; no
response or disclosure deadline is promised before a release policy is
published.
