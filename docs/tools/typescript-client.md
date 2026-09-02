# TypeScript client

`marrow client typescript` generates a strict TypeScript client for a project's
exported functions. The client launches the stock `marrow-runner` and calls
exports over a private local channel. Generator, client, supervision module, and
runner are one matched release.

## Generated files

```text
$ marrow client typescript
client/client.mts
client/marrow-supervisor.mjs
client/marrow-supervisor.d.mts
```

`--out <dir>` chooses the directory; the default is `client`.

| File | Origin | Role |
|---|---|---|
| `client.mts` | Generated per project | One named `async` method per export; exact transfer types; argument validation. |
| `marrow-supervisor.mjs` | Pinned, emitted verbatim | Process supervision, the wire codec, the serial worker, loss classification. Depends only on Node built-in modules. |
| `marrow-supervisor.d.mts` | Pinned, emitted verbatim | Type declarations so the client type-checks under strict TypeScript. |

Generation is deterministic: the same project bytes yield byte-identical output.

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

`add` is the project's `pub fn add(a: int, b: int): int`. Its arguments and
result are `bigint`, because a Marrow `int` is a 64-bit integer. `close()` hangs
up and waits for the runner to exit.

## Launching

`Client.launch` takes a `LaunchOptions` value. `runner` is the path to the
`marrow-runner` executable and `image` the path to the compiled program image.
`store`, when set, is the path to a provisioned store directory and the runner
runs the program against it; without it the session is storeless. `log` receives
the runner's drained stderr and extra stdout bytes.

The generated client carries two identities. `INTERFACE_ID` names the wire
interface; a storeless launch proves it. `IMAGE_ID` names the exact image; a
launch with `store` proves it. A runner serving any other identity is terminated
and `launch` rejects.

`terminate()` is the immediate shutdown: it kills the runner, destroys the
socket, and rejects every outstanding call with its loss class. It also runs on
process exit. `close()` is the orderly form.

`provision(options)` is a module function of `./client/marrow-supervisor.mjs`,
not a method of `Client`. It creates a store for an image. It takes `runner`,
`image`, and a `store` path that does not exist yet, and resolves with a
`ProvisionReceipt` naming the store instance and path. A store on disk needs the companion layout described under
[install](../install.md#running-against-a-store).

## Type projection

The wire carries the closed transfer graph. Its TypeScript projection:

| Marrow | TypeScript | Wire spelling |
|---|---|---|
| `int` | `bigint` | exact 64-bit integer |
| `bool` | `boolean` | `true` / `false` |
| `string` | `string` | JSON string |
| `bytes` | `Uint8Array` | `0x`-prefixed lowercase hex |
| `date`, `instant`, `duration` | `string` | the canonical text spelling |
| `T?` | `T \| null` | `null` when absent |
| `struct` | inline `{ field: T }` | object; every field is present |
| resource value | inline `{ field: T; sparse?: T }` | object; an absent sparse field is omitted |
| `enum` (including `Option`/`Result`) | `{ member: "name"; payload: [..] }` union | tagged member and dense payload |
| `List<T>` | `Array<T>` | JSON array of element values |
| `Map<K, V>` | `Array<[K, V]>` | JSON array of ordered `[key, value]` pairs, so a non-string key and entry order survive |
| `Id(^root)` | `{ readonly root: "root"; readonly key: [..] }` | JSON array of the root's key scalars; a branded handle the client cannot confuse across roots |

A returned `Map<K, V>` is an ordered `[key, value]` array; convert it with
`new Map(result)` for any key type or `Object.fromEntries(result)` for string keys.

The transfer graph is closed over every value type, so a verified signature
always projects. Arguments are validated against the export's verified signature
in the client, as a `TypeError` before any byte is sent, and again by the runner.
An export signature too complex for the fixed interface budget is a
`cli.interface_unbuildable` error at generation. A wire value nests at most 64
levels, a string carries at most 64 KiB, and a frame is at most 1 MiB; the
supervision module exports these as `MAX_DEPTH`, `MAX_STRING_BYTES`, and `MAX_FRAME`.

## Call outcomes

A call resolves with the export's value, or rejects with one of five errors.
`MarrowFault` is a runtime fault with its source position (`code`, `line`,
`column`). `MarrowIncomplete` means the invocation did not return; it carries the
same position and a `durable` state of `known_old`, `known_new`, or `unknown`, as
described under [interrupted invocations](../language/errors-and-transactions.md#interrupted-invocations).
`MarrowReject` means the runner refused the request (`runner.unknown_export`,
`runner.arg_mismatch`, `runner.durable_unsupported`). `WireFormatError` is a
wire-grammar violation (`wire.*` codes). `MarrowLossError` means the session
failed while the call was outstanding.

The supervisor terminates the session after any incomplete reply. Calls already
queued reject as `interrupted`, later calls reject as `not_started`, and none is
retried.

## Supervision and the local channel

`launch` spawns the runner without a shell, passes a fresh 256-bit launch nonce
by environment, and reads one launch-descriptor line from the runner's stdout.
By then the runner has created a mode-0700 private directory and bound a Unix
socket inside it. The supervisor connects, proves the nonce, and verifies the
session token and identity the runner sends back. One serial worker serves
requests over a bounded queue of 64 pending calls; a call beyond the quota
rejects immediately. A reply is awaited for 30 seconds, after which the session
terminates. There is no streaming, replay, cancellation, or pagination.

When the session fails with calls outstanding, each call rejects with a
`MarrowLossError` carrying one of three classes. The class is decided by how far
the call had progressed.

| Class | Meaning |
|---|---|
| `not_started` | The call did not run: launch failed, or the call was made after the session died. |
| `interrupted` | The call was queued and was not handed to the serial worker. |
| `outcome_unknown` | The call was dispatched to the runner; it may have run, and its outcome cannot be known from this side. A transport failure, malformed wire, mismatched turn, unsolicited message, or reply decode failure is retained as the `cause`. |

No class is replayed automatically, so a mutating call with an unknown outcome
runs at most once. The caller decides how to proceed.

The supervision module and the generated client speak one grammar, the canonical
wire JSON, over one transport, the private Unix socket. They use their own codec
(the global JSON codec loses 64-bit integers), open no TCP or HTTP endpoint, and
import only Node built-in modules.
