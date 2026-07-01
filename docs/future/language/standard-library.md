# Standard Library

Future counterpart of
[`../../language/standard-library.md`](../../language/standard-library.md).

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
