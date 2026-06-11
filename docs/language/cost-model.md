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

## Reading Cost From The Source

Each construct maps to a fixed shape of work:

- `^books(id).title` — one point read of one leaf cell.
- `var b: Book = ^books(id)` — one point read per stored scalar leaf in the
  record body (its fields and unkeyed groups), bounded by the schema. Keyed
  child layers are not pulled in; read them through their own paths.
- `exists(^books(id))` — one point read of the node cell.
- `for id in ^books.byShelf(shelf)` — one range scan over that index branch,
  streaming identities lazily. A field read in the loop body, such as
  `^books(id).title`, is the read you wrote: one point read per identity.
- `count(^books.byShelf(shelf))` — one range scan over the branch, not a
  maintained counter.
- `^books(id).shelf = "fiction"` — one field write plus, for each index the
  field feeds, a read of the old indexed value, removal of the old entry, and
  addition of the new entry.
- `^books(id) = book` — exact replacement: it writes the record body and clears
  every omitted field, unkeyed group, and keyed child layer.
- A bare write commits on its own; writes grouped in a `transaction` commit once.

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

## Depth Limits

Two fixed ceilings keep pathological depth from exhausting the native stack.
Both are fail-closed: the program stops with a located diagnostic, never a
process crash.

- **Nesting limit (256).** Source may nest expressions (parentheses, operators)
  and statement blocks (`if`, `while`, `for`, …) up to 256 levels deep. Deeper
  source stops at the offending span with `check.nesting_limit`.
- **Call-depth budget (256).** A running program may nest function calls up to
  256 deep. Attempting depth 257 stops at its call site with
  `run.recursion_limit`, whose payload reports both `budget=256` and the
  observed attempted depth.

Both ceilings are fixed in v0.1 and not configurable. The toolchain runs the
parse, check, and run pipeline on a worker thread with a large stack, sized so a
limit always trips before the stack can overflow — so deeply nested or unbounded
recursion is a typed diagnostic, not an abort. The v0.1 call-depth budget was
kept at 256 after measuring an optimized debugger-hook probe at a maximum
adjacent user-call stack delta of 5,040 bytes, below the 8 KiB per-frame
headroom rule for a 2 MiB minimum execution stack.
