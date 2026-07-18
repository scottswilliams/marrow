# Diagnostic voice

Marrow diagnostics are typed values. A diagnostic couples a stable dotted code, a
source location, and a rendered message; tests assert the code, span, and payload,
never the message text (see [Testing implementation](testing.md)). Because the code
is the identity, the message is free to be revised as a corpus. This page is the
normative standard for that message text: the voice every renderer sentence follows,
so a reader who has seen one diagnostic can read the next.

The standard governs prose only. It never changes which programs are accepted, which
code fires, or where it points. Those are the checker's contract; the message is how
the checker speaks it.

## The rules

A rendered message follows six rules.

1. **Facts first, in source spelling.** The first sentence states what was found,
   naming the program's own identifiers, types, and members as the source spells
   them. It does not open with a category label or a restatement of the code.
2. **Then the governing law.** The second sentence states the rule that was broken,
   in the same words the language reference uses for it. The message teaches the
   rule at the point it was met, rather than only reporting the instance.
3. **End with the fix, spelled canonically.** The final sentence gives the change to
   make, written as formatter-canonical Marrow. Because there is one way to write
   each construct and a total formatter, a message can state the fix rather than
   suggest a direction. A fix that names code uses the reader's own identifiers.
4. **No person, blame, apology, or humor.** The register is steady and impersonal —
   the register of a land registry. A message that may accompany a durable change is
   never chummy, and never scolds. There are no exclamation marks, no "you", no
   "sorry", no mascots.
5. **A runtime fault leads with what was protected.** A `run.*` fault message opens
   with the guarantee that held, before the cause. A transaction fault says the
   transaction rolled back and no data changed first; the reason follows.
6. **Codes are identity; prose is personality.** The dotted code carries the meaning
   a tool or test keys on. The prose exists for the reader and may be rewritten
   whenever a clearer sentence is found, without a contract change, so long as it
   still obeys these rules.

## Applied families

The families below are audited against the standard. Each rejection keeps its typed
code; the message is what the standard moves.

### Presence

A read or write against a possibly-absent place is refused until presence is
established. The message names the member in source spelling and states the fix.

```text
check.type — cannot assign through the possibly-absent member `note`.
A member that is not `required` is absent until it holds a value, and a
read-modify-write cannot begin from an absent place. Assign `note` a present
value first.
```

### Bound

A durable traversal is always bounded. An unbounded durable `for` head names the
missing clauses in the exact spelling that satisfies it.

```text
check.type — this durable traversal is unbounded.
A `for` head over a durable root or branch is always bounded and states its
overflow behavior. Add `at most N` and an `on more { … }` block.
```

### Transaction

A durable mutation executes only inside a `transaction` block. The message points at
the unwrapped mutation or call, cites the rule, and states the wrap.

```text
check.requires_transaction — the durable mutation here has no ambient transaction.
A durable write, replacement, or erase executes only inside a `transaction` block.
Wrap it in a `transaction` block.
```

### Match

A `match` over an enum covers every member exactly once, with no wildcard arm. The
message names the uncovered members in source spelling and states the rule before
the fix.

```text
check.match_nonexhaustive — the `match` on `Shape` does not cover `rect`.
A match covers every member of an enum exactly once and admits no wildcard arm.
Add an arm for `rect`.
```

### Method-call shape

A value takes no methods: member syntax reaches fields and constructor paths only,
and every operation on a value is a free function. A call written as a method is
rejected with the free-function spelling of the same call.

```text
check.unsupported — `trim` is written as a method call on a value.
A value has no methods; an operation on a value is an ordinary function call.
Write `trim(s)`.
```

## Enforcement

The renderer conforms to this page; the typed-code tests beside each family prove
the code, span, and payload are unchanged when the message is revised. A message
change that alters a code, a span, or which programs are rejected is a contract
change, reviewed as one — not a voice change.
