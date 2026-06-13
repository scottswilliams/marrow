# Standard Library

Marrow syntax stays small. Most everyday functionality belongs in `std::`
modules with typed signatures.

Builtins such as `exists`, `keys`, `print`, and conversions are documented in
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
need the host clock, while the parse and format helpers are pure.

The descriptor table is closed for every known `std` module. A call to an
operation the table does not define is a check error, whether the module is pure
or host-capable. Host capability only affects recognized descriptor rows such as
`std::clock::now()` or `std::io::readText(...)`.

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
std::clock::addDays(value: date, days: int): date
std::clock::daysBetween(start: date, end: date): int
std::clock::year(value: date): int
std::clock::month(value: date): int
std::clock::day(value: date): int
```

Linear instant/duration arithmetic uses operators:

```mw
const later = t + 1.hour
const elapsed = finished - started
```

Date calendar arithmetic stays in named pure helpers. `addDays` moves a `date`
by whole calendar days and raises `run.temporal_overflow` outside the supported
0001-9999 calendar range. `daysBetween(start, end)` returns `end - start` in
days. `year`, `month`, and `day` extract calendar components. These helpers do
not add date dot-fields, instant or duration overloads, month/year duration
literals, local time zone or locale behavior, or `addMonths`/`addYears`.

The host clock is captured once at the start of a run, so every `now()` call in
one run returns the same instant and `today()` the same date.

Saved `instant` values use a canonical UTC representation. The library surface
uses canonical text. `today()` returns the current UTC calendar date, not a
host-local date. Local time zone presentation and localized formatting belong
in host libraries or a separate standard-library extension, not in the
language/database kernel.

## `std::io`

Marrow `print` covers simple script output. `std::io` owns file access and
byte/text boundaries:

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
std::text::slice(value: string, start: int, end: int): string
std::text::startsWith(value: string, prefix: string): bool
std::text::endsWith(value: string, suffix: string): bool
std::text::indexOf(value: string, needle: string): maybe-present int
std::text::replace(value: string, from: string, to: string): string
std::text::join(parts: sequence[string], separator: string): string
std::text::toUpper(value: string): string
std::text::toLower(value: string): string

std::bytes::length(value: bytes): int
std::bytes::base64Encode(value: bytes): string
std::bytes::base64Decode(value: string): bytes
```

`std::text::length` counts Unicode scalar values. It does not count terminal
columns or locale-specific grapheme clusters. `std::text::slice` uses the same
zero-based scalar indexes with an exclusive `end`. `std::text::indexOf` returns
the zero-based scalar index of the first match, and is maybe-present: use `??`
or `if const` to handle no match. There is no `-1` sentinel.

Case conversion uses simple Unicode case mapping without locale-specific rules:
each input scalar maps to at most one output scalar, so mappings such as
`ß` to `SS` or `İ` to `i` plus a combining mark are not used.
`std::bytes::length` counts bytes.

## `std::math`

Numeric helpers that stay out of syntax:

```mw
std::math::absInt(value: int): int
std::math::absDecimal(value: decimal): decimal
std::math::floor(value: decimal): int
std::math::minInt(a: int, b: int): int
std::math::maxInt(a: int, b: int): int
std::math::minDecimal(a: decimal, b: decimal): decimal
std::math::maxDecimal(a: decimal, b: decimal): decimal
std::math::round(value: decimal): int
std::math::ceiling(value: decimal): int
std::math::powInt(base: int, exp: int): int
std::math::modulo(a: int, b: int): int
std::math::remainder(a: int, b: int): int
```

The `%` operator is remainder. Named functions make negative-number behavior
explicit in code that needs clarity. Separate integer and decimal names avoid a
numeric overloading rule in the language. `round` uses half-to-even. `powInt`
requires a non-negative exponent and raises the existing integer overflow fault
when the result does not fit in `int`.

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
