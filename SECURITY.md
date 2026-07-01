# Security Policy

## Supported Versions

Marrow is in the v0.1 pre-1.0 release line. Security fixes are handled on the main development line
and may ship without backports or patch releases unless a maintained release branch is explicitly
announced.

## Reporting A Vulnerability

Do not open a public GitHub issue or discussion for a suspected vulnerability.

Email reports to williamssscott@gmail.com. Include enough detail to reproduce or assess the issue:

- the affected Marrow version, commit, or branch;
- the `.mw` source, project files, store or backup inputs, commands, or host inputs involved;
- the expected behavior and the observed behavior;
- any crash output, diagnostics, logs, or proof-of-concept steps that are safe to share.

Useful reports include compiler, runtime, CLI, storage, backup/restore, and generated-client issues
that could affect confidentiality, integrity, availability, project isolation, or safe handling of
untrusted inputs.

## Response Expectations

You should receive a human response after the report has been reviewed. Follow-up may ask for a
smaller reproduction or clarification. Fix timing depends on severity, reproducibility, and the
pre-release state of the affected surface; no hard response or disclosure SLA is promised for v0.1
pre-release work.
