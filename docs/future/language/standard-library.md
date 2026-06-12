# Standard Library

Future counterpart of
[`../../language/standard-library.md`](../../language/standard-library.md).

## `std::clock`

The temporal parse helpers `std::clock::parseInstant`, `parseDate`, and
`parseDuration` exist today and accept canonical Marrow text only; see
[the current page](../../language/standard-library.md). The designed extension
makes them permissive boundary parsers that also accept the common ISO 8601 and
RFC 3339 forms used by hosts and users.

Under the extension, `parseInstant` accepts RFC 3339 date-time text with `Z` or
a numeric offset and normalizes the result to a UTC `instant`. `parseDate`
accepts ISO 8601 calendar date text. `parseDuration` accepts ISO 8601 duration
text that describes a fixed elapsed span; calendar months and years are
rejected because their length depends on an anchor date.

Storage and printing remain canonical. The `std::clock::format*` helpers return
Marrow's canonical saved text, and saved `date`, `instant`, and `duration`
values keep their canonical encodings.

## Sensitive Output

`sensitive` and `declassify` are reserved for a future checked information-flow
surface. The declassification sink set is exactly `print`, all `std::log`
functions, `std::io::writeText`, and `std::io::writeBytes`; other functions do
not become output sinks by name.

## `std::text`

`std::text::split(value, separator)` treats an empty separator as a request for
Unicode scalar pieces:

```mw
std::text::split("abc", "")   ; "a", "b", "c"
std::text::split("", "")      ; empty sequence
```

This form returns one piece per Unicode scalar value in `value`. It does not
add leading or trailing empty boundary elements, and it does not segment by
grapheme cluster or display column.

## `std::math`

Integer quotient is a named helper, not an operator:

```mw
std::math::quotient(a: int, b: int): int
```

`quotient(a, b)` returns the integer quotient paired with
`std::math::remainder(a, b)`: for nonzero `b`, `a == quotient(a, b) * b +
remainder(a, b)`. Division by zero raises the same catchable numeric fault as
other deterministic evaluator numeric failures.
