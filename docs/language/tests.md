# Tests

A `test` declaration is a named, zero-argument body that `marrow test` runs. Its
statements are ordinary Marrow, plus one construct legal only here: the `assert`
statement. A test that touches no durable data runs storeless; a test that reads
or writes durable data runs against its own fresh in-memory ephemeral attachment
(see [durable places](durable-places.md) and [tools/tests](../tools/tests.md)), so
no test opens a persistent store or observes another test's writes.

## Declaring a test

A test is the keyword `test`, a string-literal title, and an indented body:

```mw
module app::math

pub fn double(n: int): int {
    return n + n
}

test "double doubles its argument" {
    const four = double(2)
    assert four == 4
}
```

The title is a human report label. It is unique within a project and is not an
export, an entry identity, or any other stable identity. A test takes no
parameters and returns nothing.

## The assert statement

`assert <condition>` evaluates a `bool` expression. When the condition is false
the test fails, and `marrow test` reports the failure at the assertion's source
position. `assert` is legal only inside a `test` body; an `assert` in an ordinary
function is the compile error `check.assert_outside_test`. For an invariant fault
in program code, use `unreachable("...")` instead.

```mw
module app::text

pub fn shout(word: string): string {
    return word + "!"
}

test "shout appends one mark" {
    assert shout("hi") == "hi!"
    assert not isEmpty(shout(""))
}
```

A test that runs to completion with every assertion holding passes. A false
assertion fails it; any other runtime fault errors it. How `marrow test`
discovers, selects, and reports tests is described in
[tools/tests.md](../tools/tests.md).

## Durable tests

A test that touches durable data does so in one of two ways, and a single test
body uses only one of them.

A **direct** durable test reads and writes durable places itself. Its operations
run against one session over the test's fresh attachment, so a value it writes is
visible to a later read in the same body:

```mw
module docs::direct_test

resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

test "a written field reads back" {
    ^counters[1].value = 7
    assert ^counters[1].value ?? 0 == 7
}
```

A **driver** test reaches durable data only by calling the application's exports.
Each such call is its own invocation boundary, exactly as a separate terminal
invocation is: a mutating export commits to the test's attachment, and a later
reading export observes the committed value. A driver test seeds and inspects
durable state through exports rather than raw writes, and its assertions read
through reading exports:

```mw
module docs::driver_test

resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

pub fn set(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
    }
}

pub fn valueOf(id: int): int? {
    return ^counters[id].value
}

test "set then read back" {
    set(1, 42)
    assert valueOf(1) ?? 0 == 42
}
```

A test body may not combine the two: performing a durable operation directly and
also calling an export that owns a `transaction` is the compile error
`check.test_driver_mix`. The two invocation models cannot share a body — the
driven export's commit would consume the session the direct operation needs — so
split the body into a direct test and a driver test, or reach the durable data
through the exports the test drives. Only driving an export that owns a
`transaction` is restricted this way; calling a reading export that opens no
`transaction` block alongside direct durable operations is not.
