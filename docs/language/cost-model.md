# Cost Model

Marrow source names the store, the index, and the fields it reads and writes, so
the storage work a program does is written in its source. There is no hidden
access strategy to discover: the access path is the source.

This page defines what cost means in Marrow and how to read it off the code.

## Storage Operations

Cost is counted in storage operations against the engine: point reads, range
scans, writes, index-entry touches, and commits. These counts are a property of
the checked program, not of runtime statistics or a sampled run, and they are
engine-relative. They are not wall-clock time: an engine's page cache,
copy-on-write, and fsync behavior sit below this model and decide latency, but
they never change which operations a program performs.

On the native redb backend, a commit is an immediate-durability engine commit:
an unbracketed single write pays one fsync, while writes grouped in a source
`transaction` share one commit fsync.

Cost counts operation shape, not elapsed time: reads, writes, deletes, forward
and reverse scans, entries returned, bytes moved, commits, and commit fsyncs.
The only permitted typed deductions in v0.1 are:

- `key-only`: a checked loop or collection shape binds only keys/identities, so
  it may skip value materialization reads.
- `count-only`: a checked `count`/presence shape asks only for cardinality or
  presence, so it may use node/count primitives instead of reading values.

No other operation elimination is implicit. v0.1 exposes no profile flag, and
runtime measurement is not a second explanation of program meaning.

## Reading Cost From The Source

Each construct maps to a fixed shape of work:

- `^books(id).title` — one point read of one leaf cell.
- `if const b = ^books(id)` — one point read per stored scalar leaf in the
  record body (its fields and unkeyed groups), bounded by the schema. Keyed
  child layers are not pulled in; read them through their own paths.
- `exists(^books(id))` — one point read of the node cell.
- `for id in ^books.byShelf(shelf)` — one range scan over that index branch,
  streaming identities lazily. A field read in the loop body, such as
  `^books(id).title`, is the read you wrote: one point read per identity.
- `for id in ^books.byDate(start..end)` — one bounded range scan over the exact
  index prefix and trailing ordered key range, allowed only where the scan yields
  matching identities lazily.
- `for y in ^cells(1, lo..hi)` or `for pos in ^books(id).tags(lo..hi)` — one
  bounded child-key scan under the exact saved-root or keyed-layer prefix,
  streaming matching stored keys lazily.
- `count(^books.byShelf(shelf))` or `count(^books.byDate(start..end))` — one
  unbounded or bounded range scan over the branch, not a maintained counter.
- `^books(id).shelf = "fiction"` — one field write plus, for each index the
  field feeds, a read of the old indexed value, removal of the old entry, and
  addition of the new entry.
- `^books(id) = book` — exact replacement: it writes the record body and clears
  every omitted field, unkeyed group, and keyed child layer.
- A bare write commits on its own; writes grouped in a `transaction` commit once.

The checker warns with `check.commit_amplification` when a loop condition or
body contains a saved-data write outside a `transaction`, because that shape can
turn one loop iteration into one durable commit. Wrap the loop or the write in
`transaction` when the repeated writes should commit together.

The checked model records these as traversal and write facts, so tools and the
checker see the same operations the runtime performs.

Resolving absence with `??` is ordinary control flow. If the left-hand read is
absent and the default is evaluated, v0.1 constructs zero runtime `Error`
resources for that resolved absence; an `Error` value is built only when a
`catch` binds a catchable fault.

## Hidden Traversal Is A Compile Error

The one access the checker rejects is a hidden scan: a lookup with no matching
index that would walk a store the source does not visibly traverse. Marrow makes
you declare the index or write the traversal, so a scan is never discovered as a
slow path in production. Full traversal is fine when you write it
(`for id in ^books`); only a hidden one is an error.

## Minimal Storage Work

For the program as written, Marrow performs the storage operations its
semantics require. Checked lowering never adds an operation or lets runtime
statistics change which storage work the source requires.

Because the engine stores opaque ordered bytes and knows no Marrow semantics,
there is no access-strategy layer beneath the language. No lower level can
perform the same program in fewer operations: the source already names the work.

## Changing Cost Means Changing The Program

To do less work, write different code — and every lever is in the language:

- declare an `index` so a lookup is a bounded scan instead of a rejected hidden
  one;
- store the value where it is read — a redundant field, keyed child layer, or
  copy the way a hand-tuned store would keep one — so iterating yields it
  directly without a second read;
- maintain a counter when a hot count must be a point read instead of a scan;
- group writes in a `transaction` so many commits collapse into one.

These are ordinary modeling choices with visible cost. There is no lower level to
drop to for cheaper storage work, because the program names the access path.

## Depth And Breadth Limits

Fixed ceilings keep pathological depth from exhausting the native stack and a
pathological transaction from exhausting memory. All are fail-closed: the program
stops with a located diagnostic, never a process crash.

These ceilings bound depth and breadth — recursion, source nesting, and a single
transaction's staged write set — because each one maps to a finite native
resource (the call stack or buffered memory) that a runaway program would
exhaust. They are not a general step or fuel budget. Iteration is deliberately
unbounded: a `for` or `while` loop runs as many times as its source dictates,
and Marrow adds no per-loop step cap, so a `while true` with no exit runs
forever. Bounding iteration would mean inventing a fuel limit Marrow does not
have; instead, terminating loops is the program author's contract, the same as
in any general-purpose language. The asymmetry is intentional: recursion and
nesting are capped only to fail closed against stack overflow, never to ration
how much work a loop may do.

- **Nesting limit (256).** Source may nest expressions (parentheses, operators)
  and statement blocks (`if`, `while`, `for`, …) up to 256 levels deep. Deeper
  source stops at the offending span with `check.nesting_limit`.
- **Call-depth budget (256).** A running program may nest function calls up to
  256 deep. Attempting depth 257 stops at its call site with
  `run.depth`, whose payload reports the callee name, `budget=256`, and the
  observed attempted depth.
- **Transaction-breadth budget (64 MiB).** A `transaction` buffers its whole
  pending write set in memory until it commits. Once that staged write payload
  passes 64 MiB, the next write stops at its span with
  `write.transaction_too_large`. This is generous — far above any ordinary atomic
  seed or migration — and the cap trips while the buffer is still bounded, so a
  large transaction fails closed instead of being OOM-killed. Like every fault, a
  surrounding `catch` can bind it, and the aborted transaction commits nothing.
  Split an oversized atomic write into smaller transactions.

These ceilings are fixed in v0.1 and not configurable. The user-visible contract
is the diagnostic: deeply nested source, unbounded recursion, or an unbounded
transaction fails with a typed diagnostic, not a process abort.
