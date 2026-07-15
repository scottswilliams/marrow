# Tests

A `test` declaration is a named, zero-argument body that `marrow test` runs. Its
statements are ordinary Marrow, plus one construct legal only here: the `assert`
statement. Tests are storeless — a test that reads or writes durable data is
rejected at compile time.

## Declaring a test

A test is the keyword `test`, a string-literal title, and an indented body:

```mw
module app::math

pub fn double(n: int): int
    return n + n

test "double doubles its argument"
    const four = double(2)
    assert four == 4
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

pub fn shout(word: string): string
    return word + "!"

test "shout appends one mark"
    assert shout("hi") == "hi!"
    assert not isEmpty(shout(""))
```

A test that runs to completion with every assertion holding passes. A false
assertion fails it; any other runtime fault errors it. How `marrow test`
discovers, selects, and reports tests is described in
[tools/tests.md](../tools/tests.md).
