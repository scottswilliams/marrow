# Errors And Transactions

Marrow keeps failure kinds distinct and groups durable changes under one
transaction that commits or rolls back as a unit. The current language has no
throwable error value and no exception channel: a runtime fault stops the
operation and is reported with a typed code and a source span. A recoverable
failure a program handles is an ordinary `Result[T, E]` value (see
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

A mutating export owns exactly one explicit lexical `transaction` block. Every
durable write reachable from the export sits inside that block, and the export's
call graph joins it implicitly: a helper the export calls performs its writes
inside the owner's transaction but cannot open or finish one of its own. A
read-only export needs no transaction and observes one coherent snapshot for its
whole call.

```mw
module docs::transactions

resource Counter
    required value: int

store ^counters(name: string): Counter

pub fn bump(name: string)
    transaction
        const current = ^counters(name).value ?? 0
        ^counters(name).value = current + 1
```

The block stages durable writes; reads in the same transaction observe earlier
staged writes. When the block finishes, the transaction commits all staged
changes together. A required field left unset when a staged entry commits rolls
the transaction back with `run.required_missing` rather than committing a
partial entry — so several required fields may be populated across separate
statements in one transaction and validated together at commit.

The transaction ownership law is checked when the program image is verified: a
transaction is opened exactly once and committed on every path, every mutation
sits inside the region, and a transaction owner is never called. An image that
violates the law is rejected with `image.flow` before it can run.

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
