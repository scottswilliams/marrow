# Errors And Transactions

Marrow keeps failure kinds distinct and groups durable changes under one
transaction that commits or rolls back as a unit. The current language has no
throwable error value and no exception channel: a runtime fault stops the
operation and is reported with a typed code and a source span. A recoverable
failure a program handles is an ordinary `Result<T, E>` value (see
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
- **Runtime faults** occur while a verified program runs. Each carries a `run.*`
  code and is mapped to the source span of the operation that faulted — a checked
  arithmetic overflow (`run.overflow`), a zero remainder divisor
  (`run.divide_by_zero`), an exceeded text bound (`run.text_limit`), an exhausted
  execution budget (`run.budget`), a denied durable demand (`run.authority`), a
  commit that leaves a required field unset (`run.required_missing`), an
  unconfirmed commit (`run.commit`), or a store the kernel finds internally
  inconsistent (`run.corruption`). A runtime fault is **not catchable inside the
  program**; it stops the current operation.
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

The contract is enforced in two places, and this reference states where each
rule is checked today. The compiler enforces the *requires an ambient
transaction* rule at check time. The ownership lattice — one region begun once,
committed on every path, with every mutation inside it and no durable operation
after the commit — is reconstructed from the program image and enforced by the
independent verifier. A later lane (TX02) may promote parts of the lattice to
check-time diagnostics; until it lands, the split below is exact.

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
changes together. A required field left unset when a staged entry commits rolls
the transaction back with `run.required_missing` rather than committing a
partial entry — so several required fields may be populated across separate
statements in one transaction and validated together at commit.

A mutating export observes durable data *inside* its transaction, where reads see
the staged writes, and returns values it captured there. Because the commit closes
the transaction, no durable operation — read or write — may follow it; to return a
committed value, read it into a local inside the region and return that local after
the block:

```mw
pub fn setAndReport(name: string, v: int): int? {
    var reported: int? = absent
    transaction {
        ^counters[name] = Counter(value: v)
        reported = ^counters[name].value
    }
    return reported
}
```

### Transaction ownership

The ownership lattice is enforced by the independent verifier, which
reconstructs the mutation closure from the program image alone rather than
trusting the compiler. An image that violates any rule below — including a
tampered one — is rejected with `image.flow` before it can run:

- an owning export begins its transaction exactly once and commits on every
  path that returns;
- every mutation sits inside the region, and no durable operation — read or
  write, direct or through a callee — follows the commit;
- a `transaction` marker appears only inside the export that owns it: a helper
  or read-only function that carries one is rejected;
- a transaction owner is not called by another function, so its region never
  nests inside a caller's. The one exception is a test body, which drives an
  owning export as a terminal would, each call its own invocation boundary (see
  [Tests](tests.md));
- a `transaction` whose closure performs no durable operation is rejected — *a
  transaction performs no durable operation* — because it commits nothing and
  the runtime opens no session for it. A region that only reads carries read
  demand and is admitted.

### `try` inside a transaction

Prefix `try` inside a `transaction` block is ordinary control flow, not a
transaction abort. It evaluates a `Result<T, E>`, yields the value on `ok`, and
on `err` returns that `err` from the enclosing function exactly as it does
outside a region (see [Control flow](control-flow.md#prefix-try-and-transaction)).
A propagated `err` is an ordinary return value: it neither commits nor rolls the
transaction back on its own. Rollback is reserved for a runtime fault or a
confirmed abort.

Because a `try` on the `err` path returns, and because the owning export must
commit before it returns on every path, a `try` may not stand on any path that
returns before the commit — neither inside the region nor before it opens. Its
`err` return would exit while the transaction is still uncommitted. Today the
verifier reports this from the image as *a path returns without committing the
transaction* (`image.flow`); a later lane may report it at check time.

To fail a durable change deliberately, keep the single commit and make the
mutation itself conditional inside the region, then map the outcome to `err`
after the block. The region always reaches its commit; on the failure path the
mutation simply does not run, so nothing is staged and the commit persists no
change:

```mw
pub fn setPositive(name: string, v: int): Result<int, string> {
    var wrote: bool = false
    transaction {
        if v > 0 {
            ^counters[name] = Counter(value: v)
            wrote = true
        }
    }
    if wrote {
        return ok(v)
    }
    return err("value must be positive")
}
```

### Rollback and isolation

A runtime fault raised inside a transaction — a checked arithmetic fault, an
authority denial, a required-field violation at commit, or any other `run.*`
fault — rolls the whole region back before the fault reaches the caller. No
staged write of that region survives.

Each invocation is its own boundary. A faulting invocation rolls back only its
own region and leaves every earlier committed invocation intact: after one
export commits an entry, a later export that mutates and then faults before its
own commit leaves the first entry unchanged, and a reading export observes the
committed-only state. This isolation is what lets a test drive several exports
against one attachment and see each commit or rollback independently (see
[Tests](tests.md)).

## Indeterminate Commit

A pre-commit fault or a confirmed abort rolls the whole transaction back. A
commit that does not confirm is *indeterminate*: the store handle is poisoned,
every later operation fails with `run.commit`, and the process must exit and
reopen the store. On reopen a recorded commit witness classifies the store as
complete-new (the commit landed) or complete-old (it did not); the result is
reported, never retried. Local variable assignments are ordinary state and are
not rewound by durable rollback.

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
  and, on an unconfirmed commit, poisons the handle and reconciles on reopen (see
  [Indeterminate commit](#indeterminate-commit)). The broader served-execution
  and durable-programming direction is recorded under
  [`future/`](../future/served-execution.md).
