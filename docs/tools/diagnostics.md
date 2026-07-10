# Diagnostics

Marrow diagnostics use dotted codes and typed fields. Text is intended for
people; JSON and JSONL are intended for tools. The generated
[Error Code Reference](../error-codes.md) owns every registered code and its
current meaning.

## Check

```text
marrow check [--format text|json|jsonl] [--locked] <projectdir>
```

`check` parses `marrow.json`, discovers configured source and tests, parses and
statically checks them, and reports diagnostics. It also reads `marrow.lock`.
When a native store exists, check attempts a lenient read-only open so the live
accepted catalog can bind durable identities. It never creates, repairs, or
mutates the store or source tree; a locked, recovery-required, or corrupt store
is classified as present but unreadable rather than opened write-capably.

A stale lock is advisory in ordinary edit/check work. `--locked` makes a stale
lock, or a missing lock over existing durable shape, a fatal check failure for
CI. A successful write-capable `run` or `evolve apply` regenerates the lock from
the live store; check itself remains read-only.

## Doctor

```text
marrow doctor [--format text|json|jsonl] <projectdir>
```

`doctor` is read-only triage. It probes configuration, source checking,
`marrow.lock`, read-only store opening, accepted identity, source/store fences,
the engine profile, and a bounded integrity sample. It aggregates findings
instead of stopping at the first ordinary problem and exits nonzero when any
finding is present. It never repairs or rewrites the project.

Use doctor to select the next explicit action. Common findings lead to:

- `doctor.store_locked`: stop the process holding the native store;
- `doctor.store_recovery_required`: run `marrow data recover` after stopping
  writers;
- `doctor.store_unavailable`: inspect `data.underlying_code` for the underlying
  `store.*` failure;
- `doctor.integrity_sample_failed`: run the full `marrow data integrity` scan;
- `doctor.stale_lock`, `doctor.lock_missing`, or a lock mismatch: run the
  printed write-capable command that reprojects the lock.

See [Recovery](../operations/recovery.md) for the operator sequence.

## Output Formats

`--format` takes a separate value: `text`, `json`, or `jsonl` where the command
supports it. `run` supports text and JSON; backup and restore are text-only.
Trace output is text-only.

JSON emits one report object. JSONL is used when a command can stream records or
diagnostics and normally ends with a summary record. Structured diagnostics
carry at least a dotted `code` and broad `kind`; applicable reports can also
carry message, help, source location, path, and command-specific typed data.
Consumers must branch on codes and typed fields rather than human message prose.

## Execution Observation

`run --trace` and `test --trace` emit statement and managed-write events on
stderr without changing the program-output stream. `run --dry-run` executes
against isolated saved state and reports would-be managed writes; it does not
rewind external host effects. Exact option combinations are defined in the
[CLI Reference](cli.md).

## Exit Classes

Usage failures exit `2`; reached command failures generally exit `1`; successful
commands exit `0`. Some read commands deliberately treat an absent value or
missing store as a successful empty result. See [Compatibility](../compatibility.md)
for the current, pre-release interface boundary.
