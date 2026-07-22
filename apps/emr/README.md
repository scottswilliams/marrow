# EMR change-set tool

A durable Marrow application that applies bounded, atomic, order-independent
**change-sets** to a small electronic-medical-record store: patients, encounters,
observations, and medication orders, with revision discipline, reference closure,
a status-transition law, active-medication supersession, per-patient aggregate
revisions, and a contiguous audit trail. It is the campaign's dogfood port of the
syntax stress study's EMR change-set workload, built and run entirely on the stock
`marrow` toolchain against a provisioned native store.

It is **not** FHIR and accepts no FHIR resources; `revision` is an internal
counter, not a FHIR `versionId`. It adopts three ideas from FHIR R4 transaction
processing only: a change-set is atomic, its result is independent of entry order,
and a resource created in the same change-set can satisfy another entry's
reference.

## Modules

- `model.mw` — durable declarations: the `Change` bundle-entry enum, the closed
  `Rejection` reasons, the four primary resources (`Patient`, `Encounter`,
  `Observation`, `MedicationOrder`, each with a `byPatient`/`byEncounter` managed
  index), and the derived roots the change-set maintains (`patientAggregates`,
  `activeMedications`, `auditCounter`, `auditEvents`, `patientAudits`).
- `status.mw` — the status families and the create/transition law.
- `changeset.mw` — the applier. `applyChangeSet(List<Change>)` is the bundle
  entry point (tests and a generated client drive it); `applyBundle` is the shared
  ambient-transaction helper; `initStore` establishes the audit counter; and the
  `admitPatient` / `revisePatient` / `recordEncounter` / `recordObservation` /
  `orderMedication` commands are terminal-first single-resource wrappers.
- `chart.mw` — read-only chart journeys: a patient summary, index-backed counts of
  a patient's encounters/observations/active medications, and paged audit history.
- `tests.mw` — source tests (`marrow test`), driver-mode, one fresh ephemeral
  store per test.

## Journeys

- **Populate.** Import a de-identified EMR export (flat-scalar JSONL) through the
  trusted importer; the store is provisioned on first import (see *Import* below).
- **Establish.** `marrow run changeset.initStore --store <dir>` once, to create the
  audit counter.
- **Read a chart.** `marrow run chart.patientChart --store <dir> -- <patientId>`
  prints a one-line summary; `chart.countEncounters` / `countObservations` /
  `countActiveMedications` list derived views through the managed indexes;
  `chart.countAuditPage` pages a patient's audit history two links at a time.
- **Apply a change.** From a terminal, the single-resource commands
  (`changeset.admitPatient`, `revisePatient`, `recordEncounter`,
  `recordObservation`, `orderMedication`) each apply a one-entry bundle in one
  transaction. A full heterogeneous bundle is applied through
  `changeset.applyChangeSet` (from the source tests or a generated client).
- **Audit.** Every accepted mutation allocates a contiguous audit ID, writes a
  global audit event, and links each affected patient; `chart.countAuditPage`
  reads the patient's history back.

## Build, check, test

```sh
cd apps/emr
marrow check .          # clean; prints each export's durable demand
marrow test             # runs the source tests (storeless/ephemeral)
marrow fmt --check .
```

`marrow.ids` is the committed, machine-written identity ledger; commit it with the
source. If you change a durable declaration, run a storeless `marrow run` once to
mint the new identities, then commit the updated `marrow.ids`.

## Run against a native store

The native path needs the companion runner installed beside `marrow` (the stock
install layout: `marrow`, `marrow-runner`, and the `marrow-companions` release
manifest in one directory).

```sh
# populate + provision on first import (see Import), then:
marrow run changeset.initStore --store ./store
marrow run chart.patientChart --store ./store -- 1
marrow run changeset.admitPatient --store ./store -- 9001 active "New Patient"
```

## Import (the owner's real data export)

The importer populates the **flat scalar** primary roots (and the flat
`patientAggregates`) from one JSONL file per root — one JSON object per line, every
member a string, integer, or boolean, named exactly as the root's key columns and
fields. Provisioning happens automatically on the first import into a fresh store.

| Root (`--root`) | Keys (`--keys`) | Members per line |
|---|---|---|
| `patients` | `id` | `id, revision, status, display` |
| `patientAggregates` | `patient` | `patient, revision` |
| `encounters` | `id` | `id, revision, patientId, status, reason` |
| `observations` | `id` | `id, revision, patientId, encounterId, status, code, value` |
| `medicationOrders` | `id` | `id, revision, patientId, code, status, dose` |

```sh
marrow import --store ./store --jsonl export/patients.jsonl           --root patients          --keys id
marrow import --store ./store --jsonl export/patient_aggregates.jsonl --root patientAggregates --keys patient
marrow import --store ./store --jsonl export/encounters.jsonl         --root encounters        --keys id
marrow import --store ./store --jsonl export/observations.jsonl       --root observations      --keys id
marrow import --store ./store --jsonl export/medication_orders.jsonl  --root medicationOrders  --keys id
```

`corpus/generate.py` writes a referentially consistent synthetic export in this
exact shape (`python3 corpus/generate.py [PATIENTS] [OUTDIR]`); the checked-in
`corpus/*.jsonl` is a 40-patient sample. Every imported row is created through the
Marrow path kernel; the importer exposes no raw key, engine handle, or
transaction, and a store that denies writes refuses the import.

## Recorded narrowings (stock beta surface)

The change-set *semantics* are faithful to the workload contract; these
representational narrowings are forced by the current stock language and are noted
honestly rather than worked around:

- **Typed resource IDs are plain `int`.** A nominal newtype cannot be a durable
  key or a stored field on the beta line, so `PatientId`/`EncounterId`/… collapse
  to `int`; the four ID spaces are not type-distinct here.
- **Statuses are stored as `string`.** The trusted importer populates scalar
  fields only, so a primary stores its status as text; `status.mw` is the checked
  interpretation of the status families and their transitions.
- **Read-projections are managed indexes, not hand-maintained projection roots.**
  The study maintained explicit `patient_encounters`/`patient_observations`/…
  roots because its proposed language lacked managed indexes; here the `byPatient`
  and `byEncounter` indexes on the primaries serve those derived reads directly.
- **Active-medication projections are not imported.** They live under a keyed
  branch, which the flat importer does not populate, so an imported medication
  order is a baseline primary and does not occupy its `(patient, code)` active key;
  active-medication tracking and supersession begin with change-sets applied
  through the tool. Imported patients carry an aggregate revision (imported into
  `patientAggregates`) so they can be updated by later change-sets.
- **First-failure precedence is input-order sensitive.** The accept/reject
  decision, the committed state, and the audit trail (drafts are sorted into
  canonical `(kind, id, mutation-kind)` order before audit IDs are allocated) are
  all order-independent. What remains input-order sensitive is only *which*
  rejection is reported first when several entries are invalid — the contract's
  canonical smallest-subject choice needs a sort applied to the rejection scan too.
- **Active-medication supersession covers the create path only.** A newly created
  `active` order supersedes the incumbent at its `(patient, code)` key (the
  workload's signature implicit mutation, and correctly suppressed when the
  incumbent is also explicitly changed in the same bundle). Raising a *different*
  existing order to `active` at an already-occupied key via an *update* is rejected
  as an `activeMedicationCollision` rather than superseding the incumbent; model
  the swap as a create of the new active order.
- **Overflow is a runtime fault, not a domain rejection.** Revision and
  audit-counter successors are computed with plain `+ 1`; on `int` overflow they
  trap as a runtime/store fault (rolling the change-set back) rather than surfacing
  the contract's `revisionOverflow` / audit-counter-overflow domain codes. The
  aggregate-presence check assumes a coherent store (every existing patient has an
  aggregate revision, established by import of `patientAggregates` and by the
  create path).
- **The global audit event names one affected patient.** An observation whose
  patient moves affects two patients; the global `auditEvents` row records the
  primary (`patientA`), while both patient-audit links are still written, so
  per-patient audit coverage is complete.

## Design note — atomicity

A `return` inside a Marrow `transaction` commits the staged writes. The applier
therefore validates the whole bundle first (reads only) and applies it only once
every entry is valid, so any rejection returns before a single write and leaves the
store byte-for-byte unchanged.
