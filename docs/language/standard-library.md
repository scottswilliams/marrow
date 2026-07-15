# Standard Library

Standard-library functions use `std::module::function` paths. The checker has a
closed table of names and concrete signatures; an unknown module or function is
an error. Arguments bind by position. Parameter names in the signatures explain
the slots; they are not call-site labels. The current checker may accept a label
on one of these calls, after which lowering leaves the containing function
without an executable body. Do not label standard-library arguments.

Pure functions are deterministic. Host functions require the corresponding
capability from the run environment. Capability is per function: for example,
`std::clock::now()` needs a clock, while `std::clock::parseDate()` is pure.

```mw
module docs::standard_library

pub fn normalizedTitle(title: string): string
    const clean = std::text::trim(title)
    return std::text::toLower(clean)

pub fn day(text: string): date
    return std::clock::parseDate(text)
```

`use std::clock` binds `clock`, so `clock::today()` is equivalent to the fully
qualified form.

## Host Capabilities

- `clock`: `std::clock::now`, `std::clock::today`.
- `context`: `std::context::actor`, `std::context::idempotencyKey`,
  `std::context::requestId`.
- `environment`: `std::env::exists`, `std::env::get`, `std::env::require`.
- `filesystem`: `std::io::readBytes`, `std::io::readText`,
  `std::io::writeBytes`, `std::io::writeText`.
- `log`: `std::log::error`, `std::log::info`, `std::log::warn`.

## Text And Bytes

```text
std::text::length(value: string): int
std::text::trim(value: string): string
std::text::contains(value: string, needle: string): bool
std::text::split(value: string, separator: string): sequence[string]
std::text::slice(value: string, start: int, end: int): string
std::text::startsWith(value: string, prefix: string): bool
std::text::endsWith(value: string, suffix: string): bool
std::text::indexOf(value: string, needle: string): int?
std::text::replace(value: string, from: string, to: string): string
std::text::join(parts: sequence[string], separator: string): string
std::text::toUpper(value: string): string
std::text::toLower(value: string): string
std::text::urlEncode(value: string): string
std::text::urlDecode(value: string): string

std::bytes::length(value: bytes): int
std::bytes::base64Encode(value: bytes): string
std::bytes::base64Decode(value: string): bytes
std::bytes::fromText(text: string): bytes
std::bytes::toText(data: bytes): string
std::bytes::hexEncode(data: bytes): string
std::bytes::hexDecode(text: string): bytes
```

Text length and slicing count Unicode scalar values; slice indexes are
zero-based and the end is excluded. `indexOf` returns absence rather than `-1`.
Byte length counts bytes. Decode functions reject malformed input rather than
substituting data. Case conversion uses simple one-code-point Unicode mappings;
a character whose mapping expands to several code points remains unchanged.
URL encoding preserves RFC 3986 unreserved bytes, uses uppercase percent escapes,
and treats `+` literally. Base64 decoding requires canonical RFC 4648 padding.

## Hash And Identity Text

```text
std::hash::sha256(data: bytes): bytes
std::hash::sha512(data: bytes): bytes
std::hash::hmacSha256(key: bytes, message: bytes): bytes

std::id::slug(text: string): string
std::id::stableUuid(seed: string): string
```

Hash functions return raw digest bytes. `slug` produces lowercase ASCII slug
text. `stableUuid` is deterministic for a seed and returns UUID text; it does
not construct `Id(^root)`.

## Mathematics

```text
std::math::absInt(value: int): int
std::math::absDecimal(value: decimal): decimal
std::math::floor(value: decimal): int
std::math::minInt(a: int, b: int): int
std::math::maxInt(a: int, b: int): int
std::math::minDecimal(a: decimal, b: decimal): decimal
std::math::maxDecimal(a: decimal, b: decimal): decimal
std::math::round(value: decimal): int
std::math::roundDecimal(value: decimal, scale: int): decimal
std::math::ceiling(value: decimal): int
std::math::powInt(base: int, exp: int): int
std::math::modulo(a: int, b: int): int
std::math::remainder(a: int, b: int): int
std::math::quotient(a: int, b: int): int
std::math::divFloor(a: int, b: int): int
std::math::clampInt(value: int, min: int, max: int): int
std::math::clampDecimal(value: decimal, min: decimal, max: decimal): decimal
```

`remainder` and `quotient` truncate toward zero. `modulo` and `divFloor` use a
flooring quotient. Rounding is half-to-even. `roundDecimal` accepts scales from
`0` through `34`, and `powInt` requires a non-negative exponent. Invalid bounds,
division by zero, and numeric overflow raise runtime errors.

## Clock

```text
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

`now` and `today` require the host clock and are captured consistently for one
run. The parse, format, arithmetic, and component functions are pure. Instants
normalize to UTC. Durations are fixed elapsed spans; calendar months and years
are not duration units.

## JSON And CSV

```text
std::json::valid(text: string): bool
std::json::string(text: string, pointer: string): string?
std::json::int(text: string, pointer: string): int?
std::json::decimal(text: string, pointer: string): decimal?
std::json::bool(text: string, pointer: string): bool?
std::json::count(text: string, pointer: string): int?
std::json::stringLit(value: string): string
std::json::stringArray(items: sequence[string]): string

std::csv::row(cells: sequence[string]): string
std::csv::rowCount(text: string): int
std::csv::hasColumn(text: string, column: string): bool
std::csv::string(text: string, row: int, column: string): string?
std::csv::int(text: string, row: int, column: string): int?
std::csv::decimal(text: string, row: int, column: string): decimal?
std::csv::bool(text: string, row: int, column: string): bool?
```

JSON readers use RFC 6901 pointer text and return absence for a missing or null
selected value. They do not introduce a general JSON value type. CSV requires a
header row; data-row indexes are zero-based. Missing or null JSON selections and
missing CSV rows, columns, or empty cells return absence. `std::json::valid`
returns `false` for malformed or over-limit JSON. Malformed or over-limit
JSON/CSV text, malformed JSON pointers, negative CSV row indexes, and selected
values that cannot be represented as the requested scalar raise a runtime
error. `std::csv::row` is a builder and does not parse CSV input.

## Matrix

```text
std::matrix::parse(text: string): string
std::matrix::identity(size: int): string
std::matrix::rows(matrix: string): int
std::matrix::cols(matrix: string): int
std::matrix::get(matrix: string, row: int, col: int): decimal
std::matrix::add(a: string, b: string): string
std::matrix::multiply(a: string, b: string): string
std::matrix::transpose(matrix: string): string
```

Matrices use canonical string values rather than a separate type. Row and
column indexes are zero-based. Shape, parse, operation, and decimal-overflow
failures raise runtime errors. Matrix text is a rectangular sequence of decimal
rows such as `[1,2;3,4]`; input may contain whitespace, while output uses the
compact canonical form.

## Deterministic Random Values

```text
std::random::int(seed: string, step: int, min: int, max: int): int
std::random::bool(seed: string, step: int): bool
std::random::decimal(seed: string, step: int): decimal
```

These functions are deterministic over `seed` and `step`; they do not read host
entropy. `step` must be non-negative. Integer bounds are inclusive and must be
ordered. Decimal output is an integer multiple of `10^-18` in `[0, 1)`;
canonical rendering may omit trailing fractional zeroes.

## Context, Environment, And IO

```text
std::context::actor(): string?
std::context::requestId(): string?
std::context::idempotencyKey(): string?

std::env::exists(name: string): bool
std::env::get(name: string, default: string): string
std::env::require(name: string): string

std::io::readText(path: string): string
std::io::writeText(path: string, text: string)
std::io::readBytes(path: string): bytes
std::io::writeBytes(path: string, data: bytes)
```

These functions require host capabilities. Missing optional context fields
return absence; a missing required environment value or failed file operation
raises an `Error`. File writes replace the destination. IO write functions are
forbidden inside transactions; reads are permitted.

## Audit, Error, And Logging

```text
std::audit::event(action: string, actor: string, subject: string): string
std::audit::change(field: string, before: string, after: string): string

std::error::code(err: Error): string
std::error::message(err: Error): string
std::error::hasCode(err: Error, code: string): bool

std::log::info(message: string)
std::log::warn(message: string)
std::log::error(err: Error)
```

Audit functions are pure string builders and do not write data or logs.
`std::error` reads the built-in error shape. Logging requires a host sink and is
forbidden inside transactions.

## Testing

Tests are a language construct, not a standard-library module. A `test "name"`
declaration is a named zero-argument body, and `assert <condition>` is the
owned assertion legal only inside one. Tests run storeless through `marrow test`.
See [Tests](tests.md) for the declaration and the assertion, and
[`tools/tests.md`](../tools/tests.md) for how the command runs and reports them.
