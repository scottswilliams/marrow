# TypeScript Client

`marrow client typescript` generates a strict TypeScript client for a project's
exported functions, executed by the stock `marrow-runner` over a private local
channel. The generator, the generated client, the pinned supervision module, and
the runner are a matched release pair; none is compatible across releases.

## Generated files

```text
marrow client typescript [--out <dir>]
```

writes three files (default directory `client`):

| File | Origin | Role |
|---|---|---|
| `client.mts` | Generated per project | One named `async` method per export; exact transfer types; runtime validation. |
| `marrow-supervisor.mjs` | Pinned (emitted verbatim) | Process supervision, the wire codec, the serial worker, loss classification. Depends only on Node built-in modules. |
| `marrow-supervisor.d.mts` | Pinned (emitted verbatim) | Type declarations so the client type-checks under strict TypeScript. |

Generation is deterministic: the same project bytes yield byte-identical
output. The generated client pins the program's wire interface identity in
`INTERFACE_ID`; `Client.launch` refuses a runner serving any other interface.

## Type projection

The wire carries the closed transfer graph. Its TypeScript projection:

| Marrow | TypeScript | Wire spelling |
|---|---|---|
| `int` | `bigint` | exact 64-bit integer (a JS `number` is not) |
| `bool` | `boolean` | `true` / `false` |
| `string` | `string` | JSON string |
| `bytes` | `Uint8Array` | `0x`-prefixed lowercase hex |
| `date`, `instant`, `duration` | `string` | the canonical text spelling |
| `T?` | `T \| null` | `null` when absent |
| `struct` | inline `{ field: T; sparse?: T }` | object; a vacant sparse field is omitted |
| `enum` (incl. `Option`/`Result`) | `{ member: "name"; payload: [..] }` union | tagged member and dense payload |
| `List<T>` | `Array<T>` | JSON array of element values |
| `Map<K, V>` | `Array<[K, V]>` | JSON array of ordered `[key, value]` pairs (never a JS object), so a non-string key and entry order survive |
| `Id(^root)` | `{ readonly root: "root"; readonly key: [..] }` | JSON array of the root's key-column scalars; a branded handle the client cannot confuse across roots |

A returned `Map<K, V>` is an ordered `[key, value]` array; convert it with
`new Map(result)` (any key type) or `Object.fromEntries(result)` (string keys).

The transfer graph is closed over every value type, so a verified signature
always projects. Arguments are validated against the export's verified signature
both in the client (a `TypeError` before any byte is sent) and authoritatively by
the runner. (An export signature can still fail projection only if it is too
complex for the fixed interface budget or names an unknown type row, reported as
`cli.interface_unbuildable`.)

## Using the client

```ts
import { Client } from "./client/client.mts";

const client = await Client.launch({
  runner: "/path/to/marrow-runner",
  image: "/path/to/program.image",
});
const sum = await client.add(2n, 3n); // 5n
await client.close();
```

A call resolves with the export's value, or rejects with:

- `MarrowFault` — a source-mapped runtime fault (`code`, `line`, `column`);
- `MarrowIncomplete` — the invocation did not return; carries the source-mapped
  condition (`code`, `line`, `column`) and its independent `durable` state,
  `known_old`, `known_new`, or `unknown`;
- `MarrowReject` — the runner refused the request (`runner.unknown_export`,
  `runner.arg_mismatch`, `runner.durable_unsupported`);
- `WireFormatError` — a wire-grammar violation (`wire.*` codes);
- `MarrowLossError` — the session failed while the call was outstanding or no
  complete reply became available after dispatch (below).

`MarrowIncomplete` never contains a return value or a recovery witness.
`known_old` proves that the interrupted commit did not change durable state;
`known_new` proves that its proposed state was installed; `unknown` means the
runner could not establish either. The supervisor conservatively terminates the
session after any incomplete reply. Calls already queued reject as `interrupted`,
and later calls reject as `not_started`; none is dispatched or retried. A caller
that lost the reply receives `MarrowLossError("outcome_unknown")` instead and
cannot infer an internal durable classification.

## Supervision and the local channel

`launch` spawns the runner without a shell, passes a fresh 256-bit launch nonce
by environment, and reads one launch-descriptor line from the runner's stdout.
The runner has already created a mode-0700 private directory and bound a Unix
socket inside it before printing that line; the supervisor connects, proves the
nonce, and verifies the session token and interface identity the runner sends
back. Runner stdout/stderr beyond the descriptor line are drained as bytes
(never interleaved with protocol) and passed to the optional `log` callback.

Requests are served by one serial worker over a bounded queue (64 pending
calls; an over-quota call rejects immediately). Teardown — kill the runner,
destroy the socket — is explicit and also runs on process exit. There is no
streaming, replay, cancellation, or pagination.

## Loss classification

When the session fails with calls outstanding, each call rejects with a
`MarrowLossError` carrying one of exactly three classes, decided by how far the
call had progressed — never by retrying it:

| Class | Meaning |
|---|---|
| `not_started` | The call provably never ran: launch failed, or the call was made after the session died. |
| `interrupted` | The call was queued but never handed to the serial worker; it did not start. |
| `outcome_unknown` | The call had been dispatched to the runner; it may have run, and its outcome is unknowable from this side. |

No class is ever replayed automatically: a mutating call whose outcome is
unknown must not run twice. The caller decides how to proceed.

## Containment

The supervision module and the generated client speak exactly one grammar — the
canonical wire JSON the Rust wire owner defines — and one transport, the private
Unix socket. They never invoke the built-in global JSON codec (lossy for 64-bit
integers and non-canonical), never open a TCP or HTTP endpoint, and import only
Node built-in modules. A drift test keeps the emitted supervision module
byte-identical to the pinned source, and a containment test enforces the
grammar/transport bounds.
