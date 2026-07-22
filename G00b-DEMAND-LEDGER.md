# G00b + G02c demand ledger

The earned-only basis for the transfer extension. Every public export of the four
named demand sources that is currently outside the transfer graph is listed with
the exact carrier it needs. A carrier with no named caller is refused. This file
is a lane artifact, removed on integration (the completion packet carries it).

Method: the ledger was produced empirically — `marrow client typescript` was run
against each source's project; a source whose client generates today demands
nothing new; a source whose generation fails names the excluded export and the
carrier that unblocks it.

## Already-carried carriers (G00a) — recorded, no work

- **Bytes** scalar: crosses as `0x`-lowercase-hex text ↔ `Uint8Array`. Not JS.
- **Temporal** scalars (`date`/`instant`/`duration`): cross as branded canonical
  text ↔ `string` (via `marrow-temporal`). Never a JS `Date`.
- **Option / Result / user `enum`**: cross as sums.
- **Records, unit, the seven scalars**: cross.

Because Bytes and the temporal scalars were already in the G00a transfer graph and
codec, the lane adds no Bytes or temporal carrier. Base64 Bytes is **refused**: no
caller earns a second Bytes spelling beside the shipped `0x`-hex.

## Source 1 — Graph Report (`crates/marrow/tests/fixtures/v01/e07_graph_report/`)

Public exports: `report()->string`; `addNode`/`addEdge`/`removeEdge`/`setColor`/
`tint`/`setRoot` (scalar params, unit); `nodeExists->bool`; `colorOf->string?`;
`edgeWeight->int?`; `outDegree->int`. `Map`/`List` appear only in internal
(non-`pub`) functions and the private `Graph` record — none reach an export.

**Demand: NONE.** Client generates today (13 methods, verified).

## Source 2 — Club Locker (`crates/marrow/tests/fixtures/v01/club_locker/`)

Public exports use `int`/`string`/`bool`/`date`, `Result<int,string>` /
`Result<bool,string>`, and `string?`/`int?` — all already carried (`date` scalar,
`Result` sum, `Option`).

**Demand: NONE new.** Client generates today (verified: `eDate`×4, `dSum`×4,
`dOpt`×6). The exit-gate strict-compile + round-trip is owed at this lane.

## Source 3 — EMR tool (`apps/emr/`)

Public exports are otherwise scalar (`chart`, `status`, the `changeset` scalar
wrappers `admitPatient`/`revisePatient`/`recordEncounter`/`recordObservation`/
`orderMedication` → `string`; `rejectionCode(Rejection)->string` — `Rejection` is a
sum, carried). One export is excluded today:

- `changeset.applyChangeSet(changes: List<Change>): Result<ChangeSetApplied, Rejection>`
  — parameter 0 is `List<Change>`. Generation fails:
  `cli.transfer_excluded: `changeset.applyChangeSet`: parameter 0 uses a collection`.

**Demand: `List<T>`.** Named caller: `changeset.applyChangeSet`.

## Source 4 — Workshop, verbatim (G03 forward finding)

The G03 trusted-main Workshop was trimmed to scalar-only exports precisely because
the two richer exports reach types outside the G00a graph. Closing that finding:

- `holders(): Map<...>` — **Demand: `Map<K,V>`.** Named caller: Workshop `holders`.
- `findByTag(tag): Id(^…)?` — **Demand: `Identity` (`Id(^root)`).** Named caller:
  Workshop `findByTag`.

## Carriers built (earned)

1. **`List<T>`** → wire JSON array of encoded `T`; TS `T[]`. Caller: EMR
   `applyChangeSet`.
2. **`Map<K,V>`** → wire JSON array of ordered `[key, value]` pair-arrays (never a
   JS object); TS `[K, V][]`. Caller: Workshop `holders`.
3. **`Identity` (`Id(^root)`)** → wire JSON array of encoded key-column scalars; TS
   a branded `{ root; key }` handle. Caller: Workshop `findByTag`.

With these three, the transfer graph is closed over every `ImageType`, so the
`TransferTypeExcluded` exclusion path is deleted (re-homed diagnostics for the
surviving too-complex / unknown-type interface failures).

## Refused

- **base64 Bytes** — Bytes already crosses as `0x`-hex; no caller earns a second
  spelling.
