# Tests

A `test` is a named body of ordinary statements that `marrow test` runs.
Inside it, `assert` checks a condition.

## Tests and assert

A test is the keyword `test`, a string title, and a block:

```mw
module docs::tests::pure

pub fn label(title: string, author: string): string {
    return $"{title} by {author}"
}

test "label joins title and author" {
    const text = label("Small Gods", "Terry Pratchett")
    assert text == "Small Gods by Terry Pratchett"
    assert not isEmpty(text)
}
```

The title is the report label. Titles are unique within a project; a second
test with the same title is `check.name_conflict`. A test takes no parameters
and returns nothing.

`assert` evaluates a `bool` expression. A false condition fails the test, and
the report names the assertion's source position. A test passes when its body
runs to the end with every condition true. Any other runtime fault, such as an
overflow, errors it.

`assert` belongs only in a `test` body; in a function it is
`check.assert_outside_test`. Program code states an invariant with
`unreachable("...")` instead.

How tests are selected, ordered, and reported is described in
[tools/tests](../tools/tests.md).

## Durable tests

A test that calls functions with durable operations gets its own empty in-memory
store. Nothing carries over from one test to the next, and no test opens a
store on disk.

A test sets up data through transaction-owning exports and observes it through
ordinary read functions. A reader used only by the test can stay private:

```mw
module docs::tests::durable

resource Book {
    required title: string
    shelf: string
}

store ^books[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

fn present(id: int): bool {
    return exists(^books[id])
}

fn titleOf(id: int): string? {
    return ^books[id].title
}

test "a written entry reads back" {
    add(1, "Small Gods")
    assert present(1)
    assert titleOf(1) ?? "" == "Small Gods"
    assert not present(2)
}

test "add then read back" {
    add(1, "Small Gods")
    assert titleOf(1) ?? "" == "Small Gods"
}
```

Each call from the test body is an invocation. `add` commits its
[transaction](errors-and-transactions.md#transactions) to the test's store,
and `titleOf` reads the committed value. Related reads within one reader share
its read session. Helpers called by that function share its session; assertions
remain in the test body.

The test body has no durable session of its own. A direct durable read, write,
presence check or traversal is `check.test_durable_operation`. Calling a
mutating helper without a transaction owner is `check.requires_transaction`.
A `transaction` block in a test is `check.transaction_misplaced`. Put mutation
in an owning export and call it from the test. Constructing an entry identity,
such as `Id(^books, 1)`, is an ordinary value operation and remains legal.
