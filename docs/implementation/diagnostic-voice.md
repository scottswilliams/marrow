# Diagnostic voice

A diagnostic couples a stable dotted code, a source location, and a rendered
message. Tests assert the code, span, and payload, never the message text
([testing](testing.md)). Because the code is the identity, the message can be
revised as a corpus, and this page is the voice every rendered sentence follows.

The standard governs prose only. It changes neither which programs are
accepted, nor which code fires, nor where it points. Those are the checker's
contract; the message is how the checker speaks it.

## The rules

A rendered message follows six rules.

1. Facts first, in source spelling. The first sentence states what was found,
   naming the program's own identifiers, types, and members as the source
   spells them. It does not open with a category label or a restatement of the
   code.
2. Then the governing law. The second sentence states the rule that was broken,
   in the same words the language reference uses for it. The message teaches
   the rule at the point it was met.
3. End with the fix, spelled canonically. The final sentence states the change
   to make, written as formatter-canonical Marrow. Identifiers the reader
   already wrote appear in their own spelling; content the author must still
   supply (a bound size, a block body, a payload binding) appears as a
   placeholder (`N`, `{ … }`, `_`). Where the code fully determines the
   corrected form, such as a method call rewritten as a function call, it is
   spelled in full.
4. No person, blame, apology, or humor. The register is steady and impersonal.
   There are no exclamation marks, no "you", no "sorry", no mascots.
5. A runtime fault leads with what was protected. A `run.*` fault message opens
   with the guarantee that held, before the cause. A transaction fault says the
   transaction rolled back and no data changed first; the reason follows.
6. Codes are identity; prose is personality. The dotted code carries the
   meaning a tool or test keys on. The prose exists for the reader and may be
   rewritten whenever a clearer sentence is found, so long as it still obeys
   these rules.

## Applied families

The families below are audited against the standard. The CLI renders each as
one line, `path:line:column: code: message`; the later fences wrap the message
for reading.

### Presence

An assignment through a sparse member of a local value (`book.note.text = …`
where `note` is not `required`) has no present place to modify. The message
names the member in source spelling and states the fix.

```text
src/main.mw:14:5: check.type: cannot assign through the possibly-absent member `note`. A member that is not `required` is absent until it holds a value, and a read-modify-write cannot begin from an absent place. Assign `note` a present value first.
```

### Bound

A durable traversal is always bounded. An unbounded durable `for` head names the
missing clauses in the exact spelling that satisfies it.

```text
check.type: this durable traversal is unbounded.
A `for` head over a durable root or branch is always bounded and states its
overflow behavior. Add `at most N` and an `on more { … }` block.
```

### Transaction

A durable mutation executes only inside a `transaction` block. The message
points at the unwrapped mutation or call, cites the rule, and states the wrap.

```text
check.requires_transaction: the durable mutation here has no ambient transaction.
A durable write, replacement, or erase executes only inside a `transaction` block.
Wrap it in a `transaction { … }` block.
```

### Match

A `match` over an enum covers every member exactly once, with no wildcard arm.
The message names the uncovered members in source spelling and states the rule
before the fix; with several missing members it says "arms" and lists each.

```text
check.match_nonexhaustive: the `match` on `Shape` does not cover `rect`.
A match covers every member of an enum exactly once and admits no wildcard arm.
Add the missing arm: `rect(_, _) =>`.
```

### Method-call shape

A value takes no methods: member syntax reaches fields and constructor paths
only, and every operation on a value is a free function. A call written as a
method is reported with the free-function spelling of the same call.

```text
check.unsupported: `trim` is written as a method call on `s`.
A value has no methods; an operation on a value is an ordinary function call.
Write `trim(s)`.
```

### Refused declaration

A declaration the compiler refused keeps its name. Its cause is reported once,
at the declaration, and the first use of the name is steered to that report,
carrying the *declaring* code so one code leads to one fix; later uses of the
same name fail silently. The reader can see the name declared, so the message
does not call it out of scope, and it names a location only where a report
sits.

```text
check.unsupported: `limit` was declared, but its declaration was refused.
A refused declaration keeps its name and binds no value, so this use cannot
resolve. Correct the `check.unsupported` report at the declaration of `limit`.
```

Where another pass or an earlier stage made the report, the steer names the
code without claiming a location: *Correct the reported `check.recursion`.* for
a cause the value-cycle pass reports after the check, and *Correct the
`parse.syntax` reports `helper` received when it was parsed.* for a module the
parse stage refused whole.

## Enforcement

The renderer conforms to this page. The tests beside each family prove the
code, span, and payload are unchanged when the message is revised. A change
that alters a code, a span, or which programs are accepted is a contract
change and is reviewed as one.
