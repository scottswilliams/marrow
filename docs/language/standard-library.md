# Standard Library

Marrow syntax stays small. Most everyday functionality belongs in `std::`
modules with typed signatures.

Builtins such as `exists`, `keys`, `write`, and conversions are documented in
[Builtins](builtins.md). This page describes importable libraries.

## Design Rules

The standard library is part of the language surface, but it is not a second
runtime platform:

- `std::` modules do not add syntax.
- Pure helpers are deterministic.
- Host helpers depend on explicit host capabilities.
- Saved data still goes through `^`, builtins, and managed writes.
- Functions have concrete signatures; Marrow does not use overloading to make
  library calls work.

The first standard library does not include HTTP clients, process execution,
directory walking, random number generation, regular expressions, localized
formatting, JSON object mapping, or backend-specific storage APIs. Those can be
host libraries or separate extensions after the language/database kernel is
stable.

The Marrow language does not require every host module. Pure helpers are
available in normal CLI runs. Host functions in `std::clock`, `std::io`,
`std::env`, and `std::log` depend on the command or embedding host; a call made
without the matching capability is a typed capability error. The capability is
per function, not per module: within `std::clock` only `now()` and `today()`
need the host clock, while the parse, format, and `add` helpers are pure.

The pure modules (`std::math`, `std::text`, `std::bytes`, `std::assert`) are
closed: their full operation set is fixed, so a call to an operation they do not
define is a check error. Host modules (`std::clock`, `std::io`, `std::env`,
`std::log`) are not closed by this checker rule. Only recognized host helpers
are lowered to runtime capabilities; the language does not promise that an
unrecognized host operation reaches a host boundary.

## Import Style

```mw
use std::clock

const now: instant = clock::now()
```

Fully qualified calls are always valid:

```mw
const now: instant = std::clock::now()
```

## `std::clock`

Clock value types are part of the language surface:

- `date`
- `instant`
- `duration`

Basic helpers:

```mw
std::clock::now(): instant
std::clock::today(): date
std::clock::parseInstant(text: string): instant
std::clock::parseDate(text: string): date
std::clock::parseDuration(text: string): duration
std::clock::formatInstant(value: instant): string
std::clock::formatDate(value: date): string
std::clock::formatDuration(value: duration): string
std::clock::add(value: instant, span: duration): instant
```

A `duration` argument can be written as a
[duration literal](syntax.md#duration-literals) instead of parsed from text, so
`std::clock::add(t, 1.hour)` shifts an instant by one hour without
`parseDuration`.

The host clock is captured once at the start of a run, so every `now()` call in
one run returns the same instant and `today()` the same date.

Saved `instant` values use a canonical UTC representation. The library surface
uses canonical text. `today()` returns the current UTC calendar date, not a
host-local date. Local time zone presentation and localized formatting belong
in host libraries or a separate standard-library extension, not in the
language/database kernel.

## `std::io`

Marrow `write` and `print` cover simple script output. `std::io` owns file
access and byte/text boundaries:

```mw
std::io::readText(path: string): string
std::io::writeText(path: string, text: string)

std::io::readBytes(path: string): bytes
std::io::writeBytes(path: string, data: bytes)
```

IO errors raise typed `Error` values. `writeText` and `writeBytes` replace the
whole file and are not atomic. The host grants or withholds filesystem access
as a whole; Marrow does not sandbox paths.

## `std::env`

Environment and configuration access is explicit:

```mw
std::env::exists(name: string): bool
std::env::get(name: string, default: string): string
std::env::require(name: string): string
```

## `std::text` And `std::bytes`

`string` is UTF-8 text. `bytes` may contain arbitrary bytes.

Basic helpers:

```mw
std::text::length(value: string): int
std::text::trim(value: string): string
std::text::contains(value: string, needle: string): bool
std::text::split(value: string, separator: string): sequence[string]

std::bytes::length(value: bytes): int
std::bytes::base64Encode(value: bytes): string
std::bytes::base64Decode(value: string): bytes
```

`std::text::length` counts Unicode scalar values. It does not count terminal
columns or locale-specific grapheme clusters. `std::bytes::length` counts
bytes.

## `std::math`

Numeric helpers that stay out of syntax:

```mw
std::math::absInt(value: int): int
std::math::absDecimal(value: decimal): decimal
std::math::floor(value: decimal): int
std::math::modulo(a: int, b: int): int
std::math::remainder(a: int, b: int): int
```

The `%` operator is remainder. Named functions make negative-number behavior
explicit in code that needs clarity. Separate integer and decimal names avoid a
numeric overloading rule in the language.

## `std::assert` And Testing

Testing helpers work in ordinary `.mw` functions. Equality and ordering remain
normal Marrow expressions:

```mw
std::assert::isTrue(condition: bool)
std::assert::isFalse(condition: bool)
std::assert::absent(path)
std::assert::fail(message: string)
```

Write equality assertions by passing a boolean condition:

```mw
std::assert::isTrue(actual == expected)
```

`absent(path)` is the testing counterpart to `exists(path)`. It does not hide
schema or decoding errors.

`marrow test` runs every `pub fn` with no parameters in a test file as a test;
other functions are helpers. Test files are the project's `tests` patterns in
`marrow.json`. Each test runs against a fresh in-memory store, so tests are
independent and never touch saved data. `marrow test` reports failures as typed
`Error` values with source locations.

## `std::log`

Logging writes to host-configured sinks:

```mw
std::log::info(message: string)
std::log::warn(message: string)
std::log::error(err: Error)
```

Application audit data is saved explicitly as resources when it must be
queryable.
