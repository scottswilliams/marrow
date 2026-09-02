# Tests

A `test` is a named body of ordinary statements that `marrow test` runs. Inside it, `assert` checks a condition.

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

The title is the report label. Titles are unique within a project; a second test with the same title is `check.name_conflict`. A test takes no parameters and returns nothing.

`assert` evaluates a `bool` expression. A false condition fails the test, and the report names the assertion's source position. A test passes when its body runs to the end with every condition true. Any other runtime fault, such as an overflow, errors it.

`assert` belongs only in a `test` body; in a function it is `check.assert_outside_test`. Program code states an invariant with `unreachable("...")` instead.

How tests are selected, ordered, and reported is described in [tools/tests](../tools/tests.md).

## Durable tests

A test that reads or writes a durable place gets its own empty in-memory store. Nothing carries over from one test to the next, and no test opens a store on disk.

A durable test works in one of two ways. A direct test reads and writes durable places itself:

```mw
module docs::tests::direct

resource Book {
    required title: string
    shelf: string
}

store ^books[id: int]: Book

test "a written entry reads back" {
    ^books[1] = Book(title: "Small Gods")
    assert exists(^books[1])
    assert ^books[1].title ?? "" == "Small Gods"
    assert not exists(^books[2])
}
```

The write is a bare statement; a test body owns no `transaction` block, and one inside it is `check.transaction_misplaced`. A value the body writes is visible to a later read in the same body. A test seeds the data it needs the same way, in its own body first.

A driver test reaches durable data only through the project's exports:

```mw
module docs::tests::driver

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn titleOf(id: int): string? {
    return ^books[id].title
}

test "add then read back" {
    add(1, "Small Gods")
    assert titleOf(1) ?? "" == "Small Gods"
}
```

Each call behaves like a separate `marrow run`. `add` commits its [transaction](errors-and-transactions.md#transactions) to the test's store, and `titleOf` reads the committed value.

A body is either direct or driver. Mixing a direct durable operation with a call to an export that owns a `transaction` is `check.test_driver_mix`; split such a test in two. A direct test may still call an export that opens no `transaction`, such as a reading export.
