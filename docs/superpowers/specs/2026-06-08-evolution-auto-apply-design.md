# Evolution auto-apply on run — design

## Problem

`docs/data-evolution.md` lists adding a sparse field as a "source change only — existing
records stay valid." In practice a bare `marrow run` over a *populated* store after a
source-only sparse add fails closed with `run.schema_drift`: the source digest binds the
resource's member shape, the shape changed, and the activation fence rejects the same-epoch
re-run. The only repair today is a full `evolve preview` → `evolve apply` cycle, even though no
data migration is required. The doc promises a re-run that the engine refuses.

This was surfaced by the gap-test wave (ignored divergence test
`scenario_evolve_write_run_cli.rs::a_bare_run_after_a_source_only_sparse_add_is_fenced_by_schema_drift`,
backlog B20).

## Decision

`run` auto-applies an evolution **iff, under the apply lock, discharging it mutates zero stored
records.** Everything else stays behind explicit `evolve apply`; destructive drops against
populated data additionally require confirmation. The digest and fence are unchanged — the
digest still binds full member shape and still arbiters per-epoch schema. We are not weakening
the fence; we are letting `run` perform the apply automatically for the subset where apply is a
metadata-only stamp.

### Rejected alternative

Making a sparse add *not* advance the digest is unsound: two binaries with different schemas
would share one epoch and write a forked store; the fence would lose its per-epoch schema-arbiter
role once the evolve block is consumed; and crash-resume would break the invariant that a binary
which just wrote always passes its own fence. The digest must keep binding shape. Auto-apply
preserves all of this because it performs the real apply (new digest, advanced epoch).

## The predicate

When the fence detects schema drift at the current epoch, the runtime computes the evolution
obligation against live data **inside the apply transaction** and classifies:

| Change | Obligation | Behavior |
|---|---|---|
| Sparse field add; new resource/store/enum-member/const; any change against an **empty** affected store | none (zero records to mutate) | **auto-apply on run**: stamp new digest + epoch, proceed |
| Required field add with default against a **populated** store | backfill N records | fence with actionable diagnostic; `evolve apply` discharges it |
| Transform / type change that rewrites records | rewrite records | explicit `evolve apply` |
| Drop field/resource/store where the target is **empty/unpopulated** | none | auto-apply on run |
| Drop field/resource/store against **populated** data | destructive (data loss) | explicit `evolve apply` **and** confirmation |
| Any evolution flagged `evolve.approval_required` | — | never auto-applies, regardless of obligation |

Emptiness is not a special case; it *discharges the obligation*. An empty store has no records to
backfill or lose, so any change against it falls into the zero-mutation bucket automatically. The
same source edit therefore auto-applies against an empty store and fences (directing to `evolve
apply`) against a populated one that needs backfill — the intuitive, correct behavior.

Adds and deletes differ: population creates a *mechanical backfill* obligation for adds, but makes
a drop *destructive*. Destructive drops stay explicit and confirmed even when "valid," because
losing data must never be a silent side effect of `run`.

## Correctness: the probe must be atomic with the stamp

Population is a live fact. Probing "is the store empty" in a separate step and stamping later is a
time-of-check/time-of-use hole: a concurrent insert in between would let `run` auto-migrate a
store that is no longer empty. The obligation probe and the stamp therefore happen in **one
transaction under the write lock**. Apply is already transactional; the auto-apply decision moves
*inside* the fence/apply transaction — at run start, compute the obligation against committed
data, and if zero, stamp-and-proceed; otherwise fence. A write that commits after the probe must
serialize so the decision reflects the state at stamp time.

`run` advancing the epoch fences other bindings still on the old epoch (`StoreEvolved`). This is
intended and is the same lockout an explicit `evolve apply` produces; it is documented, not a
regression.

## Cost

The auto path needs only cheap checks: "is the change intrinsically additive?" (static) or "is the
affected store empty?" (an index count/exists). The expensive check — "is field X populated on any
record?", needing an index or scan — is only relevant to drops, which stay explicit regardless. So
the run hot path never pays for a field-population scan.

## Scope of change

- `marrow-run` run-startup path: when the fence detects drift, compute the evolution witness
  (the existing preview/obligation computation) inside the apply transaction and branch on the
  predicate. Key files: `crates/marrow-run/src/evolution/window.rs` (fence),
  `crates/marrow-run/src/evolution/apply.rs` (stamp + obligation staging), the run entry path.
- `docs/data-evolution.md` in lockstep: rewrite the "add a sparse field" row and any other
  "source change only" rows (the enum-member-reorder row has the same latent gap) to state the
  run-time auto-apply behavior and exactly when explicit `evolve apply` is still required.
- No change to `source_digest.rs` (digest keeps binding full shape) and no change to the fence's
  digest comparison; the new behavior is the run path's response to a fence miss.

## Tests (typed oracles, production pipeline)

- Re-pin the ignored divergence test: a bare run after a sparse add **auto-applies** — exit 0, old
  records read back intact, the new field is absent on old records, the epoch advanced by one.
- Empty-store: a *required*-field add against an empty store auto-applies on run (obligation
  discharged by emptiness); the same add against a populated store still fences and `evolve apply`
  backfills it.
- Destructive drop: a drop against populated data fences/stays explicit and requires confirmation;
  a drop whose target is empty auto-applies.
- Approval-required evolution never auto-applies even at zero obligation.
- Concurrency/TOCTOU: a write that races the auto-apply probe cannot produce an unsound stamp; the
  decision reflects committed state at stamp time.
- Crash-resume: a binary that just auto-applied passes its own fence on resume (no spurious drift).

## Soundness review focus (adversarial lens)

The in-transaction atomicity of probe + stamp; that no change kind which silently mutates records
can slip into the zero-mutation bucket; that destructive drops never auto-apply against populated
data; that the epoch/crash-resume invariants hold; that the approval gate is never bypassed; that
no two schemas can share one epoch.
