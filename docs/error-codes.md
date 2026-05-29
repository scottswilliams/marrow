# Errors

Marrow errors are part of the product surface. A good error says what
happened, where it happened, and what to try next when Marrow knows.

Language-level error behavior is described in
[`language/control-flow-and-effects.md`](language/control-flow-and-effects.md).
This page describes the CLI and tooling contract.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, capability, runtime, storage, project, or tool failure. |
| `2` | Command-line usage failed before the command body ran. |

## Error Envelope

Machine-readable surfaces use a stable envelope:

```json
{
  "code": "parse.syntax",
  "kind": "parse",
  "message": "expected expression",
  "help": "Add an expression after return.",
  "source_span": {
    "file": "src/app.mw",
    "line": 12,
    "column": 16
  }
}
```

The envelope is a tooling representation of an error. In `.mw` code, thrown
errors are `Error` values as described in the language reference. Tools may
add fields such as `kind` and `source_span` when reporting the error outside
the running program.

Common fields:

- `code`: stable machine code;
- `kind`: broad category such as `parse`, `check`, `capability`, `runtime`,
  `storage`, `io`, `usage`, `protocol`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use stable lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

Marrow surfaces use dotted Marrow error codes and typed error values.

Storage errors include the failed operation, a safe path or prefix when one is
available, and the capability or limit involved. Machine-readable facts belong
in `data`; clients do not parse `message`. The store reports a `store.*` code:
`store.io`, `store.locked`, `store.format_version`, `store.corruption`,
`store.limit`, and `store.corrupt_path`. Backends enforce no key or value size
limit, so `store.limit` is produced only by archive framing, when a record's
length exceeds the archive's `u32` chunk-length field.

Managed-root protection raises `write.*` codes when code attempts maintenance
work without the maintenance capability: `write.requires_maintenance` for a
whole managed-root delete, and `write.raw_requires_maintenance` for a raw
quoted-segment write or read under a managed root. The latter is distinct from
`write.unknown_field` so a tool can tell raw syntax from a declared-field typo.

The `marrow serve` data server reports a `protocol.*` code when a request is bad:
`protocol.malformed` (not JSON, or no `op`), `protocol.unknown_op`, and
`protocol.bad_request` (malformed operation arguments — a missing or bad `path`,
an unknown path segment or key type, or invalid base64). A request that reaches
the store carries the store's own `store.*` code through unchanged.

`marrow data integrity` reports `data.*` codes (kind `tooling`) for the findings
it surfaces while verifying saved data against the project schema:
`data.decode` for a stored value that is not a canonical form of its declared
type, and `data.orphan` for saved data under an unknown root or naming a member
the schema does not declare. An undecodable stored key it meets is surfaced with
the store's own `store.corrupt_path`. A `data` or `fmt` command run against a
project with a missing or invalid `marrow.json` reports the `config.*` family
`load_config` already produces.
