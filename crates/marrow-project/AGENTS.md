# marrow-project Contributor Notes

This crate is the pure project-input owner: the closed `marrow.toml` manifest
schema, deterministic contained discovery over caller-supplied file listings and
bytes, root-relative canonical file identities, path-derived module names, the
`marrow.ids` durable-identity ledger (entropy-minted ids, `(kind, path)`
anchors, tombstones, and the canonical machine-written artifact), and the
immutable `ProjectInput`. It is the one owner of that boundary. The ledger
model here is pure: entropy is drawn and the artifact is published only by the
CLI, and the compiler consumes the parsed ledger read-only.

Keep it pure: no filesystem, Git, network, compiler, runtime, or store edge. The
physical adapter that walks `src`, reads bytes, and enforces the capture limits
lives in the `marrow` CLI crate and feeds this owner, which validates its input
and rechecks the bounds. Discovery here is a total function of its inputs, so the
same files yield a byte-identical `ProjectInput` regardless of order or location.

Module identity is derived once from the canonical source path; there is no
in-source module header and no single-file fallback. Diagnostics are typed
variants carrying a stable `marrow-codes` string; manifest faults share
`config.invalid`, discovery faults use the `project.*` family. The dependency on
`toml` is parse-only and closed-schema: unknown keys reject rather than being
ignored.
