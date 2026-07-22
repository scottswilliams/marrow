# Errors And Transactions

Marrow keeps failure kinds distinct and groups durable changes under one
transaction that commits or rolls back as a unit. The current language has no
throwable error value and no exception channel: a runtime condition stops the
operation and is reported with a typed code and a source span. When a durable
invocation did not return and a commit boundary matters, the outcome also carries
an independent durable-state classification; that classification is not a return
value. A recoverable failure a program handles is an ordinary `Result<T, E>` value (see
[Types and values](types-and-values.md)), returned explicitly and propagated
with prefix `try`; the language `Result` is none of the four failure kinds
below.

## Failure Kinds

A program can fail in four distinct ways, and they never collapse into one
channel:

- **Source diagnostics** are reported before the program runs, when the source
  does not parse or check. They carry a `parse.*` or `check.*` code and a span.
- **Artifact rejection** happens when the independent verifier refuses a program
  image (an `image.*` code). A compiler cannot mint a verified image; only
  verification produces one, so a malformed or tampered image is rejected before
  the VM runs it.
- **Runtime conditions** occur while a verified program runs. Each carries a `run.*`
  code and is mapped to the source span of the operation that faulted — a checked
  arithmetic overflow (`run.overflow`), a zero remainder divisor
  (`run.divide_by_zero`), an exceeded text bound (`run.text_limit`), an exhausted
  execution budget (`run.budget`), a denied durable demand (`run.authority`), a
  commit that leaves a required field unset (`run.required_missing`), a commit
  that does not complete normally (`run.commit`), or a store the kernel finds
  internally inconsistent (`run.corruption`). An ordinary runtime fault says no
  commit was confirmed by that invocation. If a commit boundary was reached or a
  prior commit in the same invocation was confirmed, the outcome is instead
  **incomplete** and pairs the code and span with `known_old`, `known_new`, or
  `unknown` durable state. Neither form is catchable inside the program; both stop
  the current operation.
- **Operational errors** are owner-local failures of the command or store itself
  (a missing project, an I/O failure, a `store.*` condition). They are not
  program values.

See [Error codes](../error-codes.md) for the full typed list.

## Transactions

Only an export owns a `transaction` region, and a mutating export owns exactly
one. Every durable write reachable from the export sits inside that block, and
the export's call graph joins it implicitly: a helper the export calls performs
its writes inside the owner's transaction but cannot open or finish one of its
own. A read-only export needs no transaction and observes one coherent snapshot
for its whole call. It may still open a `transaction` block — a read-only region,
whose reads are admitted inside it — though it gains nothing by doing so; as in any
owned region, no durable operation may follow the block's commit.

The contract is enforced in two places. The compiler enforces every rule below
at check time, reporting a typed `check.*` diagnostic at the offending source
construct: the *requires an ambient transaction* rule, and the ownership lattice —
one region begun once, committed on every path, with every mutation inside it and
no durable operation after the commit. The independent verifier reconstructs the
same lattice from the program image alone and remains the boundary: a malformed or
tampered image that reaches it is rejected with `image.flow` before it can run. A
program a compiler accepts therefore also verifies; the check-time diagnostics are
the earlier, source-facing report of the laws the verifier guarantees.

### Requiring an ambient transaction

A function that mutates durable state carries a checked *requires an ambient
transaction* requirement. A durable write, replacement, or erase — or a call to
a function that itself mutates — is accepted only inside a `transaction` block,
or in a function whose caller supplies one. Performing such a mutation, or
calling such a helper, directly in an export body with no enclosing
`transaction` is a `check.requires_transaction` error reported at the mutation
or call-site span. The requirement propagates transitively along the acyclic
call graph, so a helper that calls a mutating helper carries it in turn; a
read-only function carries no such requirement. The requirement is reported only
at an export entry, where no caller can supply the region; a mutating helper is
left alone because it runs inside its caller's transaction. A test entry is
likewise exempt: its ambient transaction comes from the test harness (see
[Tests](tests.md)).

```mw
module docs::transactions

resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn bump(name: string) {
    transaction {
        const current = ^counters[name].value ?? 0
        ^counters[name].value = current + 1
    }
}
```

### Staging and commit

The block stages durable writes; reads in the same transaction observe earlier
staged writes. When the block finishes, the transaction commits all staged
changes together. A required field left unset at commit is detected before the
engine transaction commits, so the invocation reports `run.required_missing`
with durable state `known_old` rather than committing a partial entry. Several
required fields may therefore be populated across separate statements in one
transaction and validated together at commit.

A mutating export observes durable data *inside* its transaction, where reads see
the staged writes. A region's commit sites are its exits: each `return` written
inside the region and the region's closing brace. An in-region `return` evaluates
its expression first — so a durable read in the returned value runs before the
commit and sees the staged writes — then commits the region's staged writes, then
returns. The closing brace commits the fall-through path. Because the commit closes
the transaction, no durable operation — read or write — may follow it on that path.
A committed value is returned directly from inside the region. Here every path
returns from inside the region, so the region has no fall-through and needs no
trailing return:

```mw
module docs::staging

resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn setAndReport(name: string, v: int): int? {
    transaction {
        ^counters[name] = Counter(value: v)
        return ^counters[name].value
    }
}
```

### Transaction ownership

The compiler enforces the ownership lattice at check time, and the independent
verifier reconstructs the same mutation closure from the program image alone
rather than trusting the compiler. Each rule below is reported at its source
construct with a typed `check.*` code; an image that violates the rule — including
a tampered one — is independently rejected with `image.flow` before it can run:

- an owning export begins its transaction exactly once
  (`check.transaction_reopened` on a second region) and commits on every path
  that returns (`check.transaction_uncommitted` on a path that exits without
  committing); its commit sites are the region's exits — each in-region `return`
  and the closing brace — and only the owning export's returns commit, not a
  helper's return inside its own frame;
- every mutation sits inside the region, and no durable operation — read or
  write, direct or through a callee — follows the commit on any path
  (`check.durable_after_commit`);
- a `transaction` marker appears only inside the export that owns it: a helper
  or `test` body that carries one is rejected (`check.transaction_misplaced`);
- a transaction owner is not called by another function
  (`check.transaction_owner_called`), so its region never nests inside a
  caller's. The one exception is a test body, which drives an owning export as a
  terminal would, each call its own invocation boundary (see [Tests](tests.md));
- a `transaction` whose closure performs no durable operation is rejected
  (`check.transaction_empty`) because it commits nothing and the runtime opens no
  session for it. A region that only reads carries read demand and is admitted.

### `try` inside a transaction

Prefix `try` inside a `transaction` block is ordinary control flow, not a
transaction abort. It evaluates a `Result<T, E>`, yields the value on `ok`, and
on `err` returns that `err` from the enclosing function exactly as it does
outside a region (see [Control flow](control-flow.md#prefix-try-and-transaction)).
A propagated `err` is an ordinary return value: it neither commits nor rolls the
transaction back on its own. Rollback is reserved for a runtime fault or a
confirmed abort.

An explicit `return` inside the region is a commit site: it commits the staged
writes before it returns. A `try`'s `err` exit is different — it is an implicit
exit, not a spelled `return`, and carries no commit. A `try` may therefore not
stand on any path that would exit the function from inside the region, nor before
the region opens while one is owed: its implicit `err` exit would leave the
transaction uncommitted. The compiler reports this at check time as
`check.transaction_uncommitted`, at the `try`; the verifier independently
reconstructs the same flow from the image and rejects it as *a path returns
without committing the transaction* (`image.flow`). A
[`require` guard](control-flow.md#require-guards) follows exactly the same law:
its failure exit is implicit and carries no commit, so a `require` on such a
path is reported the same way, at the `require`. A helper called inside the
region owns no region, so its `try` and `require` exits stay ordinary control
flow.

To fail a durable change deliberately, spell the exit as a `return`. A guard that
returns `err` before staging anything commits an empty region and persists no
change:

```mw
module docs::deliberate_failure

resource Counter {
    required value: int
}

store ^counters[name: string]: Counter

pub fn setPositive(name: string, v: int): Result<int, string> {
    transaction {
        if v <= 0 {
            return err("value must be positive")
        }
        ^counters[name] = Counter(value: v)
    }
    return ok(v)
}
```

Because an in-region `return` is a commit site regardless of the value it returns,
a `return err(...)` placed *after* a staged write commits that write. Put the guard
before the mutation, as above, to leave nothing staged on the failure path.

### Rollback and isolation

A runtime fault raised before a commit is confirmed — a checked arithmetic
fault, an authority denial, or another `run.*` fault — rolls the region back
before the fault reaches the caller. A required-field violation and a confirmed
engine abort are likewise `known_old`; no staged write of that region survives.
Once a commit is confirmed, a later pure instruction or helper fault cannot make
the invocation complete or undo that durable change. It reports an incomplete
invocation with `known_new`, retaining the later fault's code and source span.

Each invocation is its own boundary. A faulting invocation rolls back only its
own region and leaves every earlier committed invocation intact: after one
export commits an entry, a later export that mutates and then faults before its
own commit leaves the first entry unchanged, and a reading export observes the
committed-only state. This isolation is what lets a test drive several exports
against one attachment and see each commit or rollback independently (see
[Tests](tests.md)).

## Incomplete Invocation And Commit Recovery

Invocation completion and durable state are independent. `known_old` proves that
the interrupted transaction did not change durable state. `known_new` proves
that its proposed durable state was installed. `unknown` means neither state
could be established. None means the function returned, and none supplies a
return value.

A required-field or other commit-reconciliation failure, or a confirmed engine
abort, leaves the transaction known old. An ordinary runtime fault before the
commit boundary instead rolls the staged transaction back without producing an
incomplete outcome. If the engine cannot say whether a commit landed, the active handle is poisoned
and creates one opaque, non-copyable recovery fact containing the exact before
and proposed-after witness states. The lifecycle keeps the store's owner lock,
marks its descriptor unclean, closes the engine, freshly reopens the existing
engine file at the retained path, and audits it before consuming that fact. A
missing, malformed, unstamped, or unreadable engine file is never created or
adopted and yields `unknown`. An exact after-state is `known_new`; the exact
before-state is `known_old`; a different or malformed state, scope mismatch,
read failure, reopen failure, or audit failure is `unknown`.

A known recovery classification returns a fresh usable store owner, but bytecode
is not resumed and the interrupted invocation remains incomplete. `unknown`
retires the attached owner, leaves its descriptor unclean, and retains the owner
lock until process exit. Dropping the opaque fact without classification takes
the same quarantine path. The wire reports the code,
source span, and durable state without exposing witness bytes. A client that
loses the reply observes only `run.outcome_unknown`, regardless of any internal
classification. No path retries or replays application code. Local variable
assignments are ordinary state and are not rewound by durable rollback.

The owner lock excludes cooperating Marrow processes but does not authenticate
the engine file against out-of-band replacement. A structurally valid foreign
store or exact prior snapshot substituted at the retained path is not currently
distinguishable at this boundary. No file-metadata approximation is treated as a
proof; substitution and rollback detection is not a current guarantee.

## Transaction Scope

A transaction governs Marrow durable writes only. It is not a process lock and
does not make an external service part of the commit. Store conditions that
escape the block — a lock conflict, an I/O failure — surface as the
corresponding typed fault and roll the staged writes back.

## Future and reserved

The following are recorded direction, not current syntax or behavior. They name
vocabulary the durable model reserves; none introduces a spelling a program can
write today.

- A function's durable access is described as a demand — the *reads* and *writes*
  a call may perform — carried through the compiler and intersected at one
  authority-checking path kernel. The current line reports the requires-ambient-
  transaction requirement and reconstructs the mutation closure at verification;
  a check-lane obligation to surface the per-function demand as a first-class,
  stated fact is deferred.
- Read coherence and concurrent-execution properties — snapshot scope under
  concurrency, serialization conflicts, retryability, and idempotency — are
  future direction. The current line runs one attempt with no automatic retry
  and resolves an indeterminate commit from the current witness under the
  continuously held owner lock (see
  [Incomplete invocation and commit recovery](#incomplete-invocation-and-commit-recovery)). The broader served-execution
  and durable-programming direction is recorded under
  [`future/`](../future/served-execution.md).
