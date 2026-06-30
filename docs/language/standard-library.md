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

The Marrow language does not require every host module. Pure helpers are
available in normal CLI runs. Host functions in `std::clock`, `std::context`,
`std::io`, `std::env`, and `std::log` depend on the command or embedding host;
a call made without the matching capability is a typed capability error. The
capability is per function, not per module: within `std::clock` only `now()`
and `today()` need the host clock, while the parse and format helpers are pure.

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

Saved `instant` values use a canonical UTC representation. `parseInstant` and the
`instant(text)` constructor accept standard RFC-3339 input — trailing-zero
fractional seconds and an explicit numeric offset (`Z`, `+00:00`, or any
`±HH:MM`) — and normalize to the canonical UTC value, so a non-UTC offset is
shifted to its equivalent UTC instant. `parseDuration` and `duration(text)`
accept the time-based ISO-8601 subset `PnDTnHnMnS`: optional days, then `T` and
optional hours, minutes, and seconds, with a fraction allowed only on the seconds
component. Each unit is an exact fixed span (`1D` = 86400s, `1H` = 3600s,
`1M` = 60s), summed and normalized to canonical `PT<seconds>S`. A Marrow duration
is pure signed nanoseconds with no calendar or DST arithmetic, so nominal year and
month components (`P1Y`, or a date-position `M`) are rejected as
calendar-ambiguous — use days, hours, minutes, and seconds, matching `addDays`
without `addMonths`/`addYears`. Output always uses the canonical trimmed text;
`today()` returns the current UTC calendar date, not a host-local date. Local time
zone presentation and localized formatting belong in host libraries, not in the
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
std::text::indexOf(value: string, needle: string): maybe int
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

`std::text::length` counts Unicode scalar values. It does not count terminal
columns or locale-specific grapheme clusters. `std::text::slice` uses the same
zero-based scalar indexes with an exclusive `end`. `std::text::indexOf` returns
the zero-based scalar index of the first match, and is maybe-present: use `??`
or `if const` to handle no match. There is no `-1` sentinel.

Case conversion uses simple Unicode case mapping without locale-specific rules:
each input scalar maps to at most one output scalar, so mappings such as
`ß` to `SS` or `İ` to `i` plus a combining mark are not used.
`std::bytes::length` counts bytes.

`urlEncode` is RFC 3986 percent-encoding of the UTF-8 bytes: it keeps `A-Z`,
`a-z`, `0-9`, `-`, `.`, `_`, and `~` literal and writes every other byte as `%`
plus two uppercase hex digits, so a space becomes `%20`, never `+`. `urlDecode`
accepts either hex case, treats `+` as a literal plus, and raises `run.type` on a
malformed escape or on decoded bytes that are not valid UTF-8.

`fromText` and `toText` cross the UTF-8 boundary; `toText` raises `run.type` on
invalid UTF-8 rather than replacing bytes. `hexEncode` emits lowercase hex;
`hexDecode` accepts either case and raises `run.type` on odd length or any
non-hex character.

## `std::hash`

Cryptographic digests over `bytes`, returning `bytes`:

```mw
std::hash::sha256(data: bytes): bytes
std::hash::sha512(data: bytes): bytes
std::hash::hmacSha256(key: bytes, message: bytes): bytes
```

`sha256` and `sha512` return the 32- and 64-byte digests. `hmacSha256` is
HMAC-SHA256 per RFC 2104 over the 64-byte block: a key longer than the block is
first replaced by its SHA-256 digest. Pair these with `std::bytes::hexEncode` or
`std::bytes::base64Encode` to render a digest as text.

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

The `%` operator is remainder, and `/` always yields a decimal, so integer
division is a named helper rather than an operator. `/` rounds an inexact
quotient half-to-even into the 34-digit decimal envelope; only a quotient whose
magnitude leaves the envelope raises `run.decimal_overflow`. `quotient` truncates toward
zero and pairs with `remainder` (and `%`): for nonzero `b`,
`a == quotient(a, b) * b + remainder(a, b)`. `divFloor` floors toward minus
infinity and pairs with `modulo`: `a == divFloor(a, b) * b + modulo(a, b)`. The
two diverge only on operands of opposite sign that do not divide evenly, where
`divFloor` rounds one further toward minus infinity (for example
`quotient(-7, 2)` is `-3` while `divFloor(-7, 2)` is `-4`). Both raise
`run.divide_by_zero` when `b` is zero and the existing integer overflow fault
for the lone non-representable result `quotient(-9223372036854775808, -1)`.

Named functions make negative-number behavior explicit in code that needs
clarity. Separate integer and decimal names avoid a numeric overloading rule in
the language. `round` uses half-to-even. `powInt` requires a non-negative
exponent and raises the existing integer overflow fault when the result does not
fit in `int`.

`roundDecimal(value, scale)` uses half-to-even rounding to the requested
fractional precision, then returns the canonical decimal value. `scale` must be
in `0..=34`; values outside that range raise `run.type`. The result is still
canonical decimal data, so trailing zeroes are not preserved for presentation.
`clampInt` and `clampDecimal` require `min <= max` and return the nearest bound
when `value` is outside the inclusive range.

## `std::json`

JSON helpers are scalar readers over JSON text. They do not expose a JSON value
type and do not map JSON objects to Marrow resources:

```mw
std::json::valid(text: string): bool
std::json::string(text: string, pointer: string): maybe string
std::json::int(text: string, pointer: string): maybe int
std::json::decimal(text: string, pointer: string): maybe decimal
std::json::bool(text: string, pointer: string): maybe bool
std::json::count(text: string, pointer: string): maybe int
std::json::stringLit(value: string): string
std::json::stringArray(items: sequence[string]): string
```

`stringLit` renders one correctly escaped JSON string literal, including the
quotes. `stringArray` renders a JSON array of those literals; an empty sequence
renders as `[]`.

`pointer` is an RFC 6901 JSON Pointer. The empty string selects the root; other
pointers start with `/`, split on `/`, and decode `~0` to `~` and `~1` to `/`.
Missing members, missing indexes, and JSON `null` are absent and compose with
`??`. Malformed pointers, malformed JSON for scalar reads, wrong scalar kinds,
duplicate object keys, leading-zero array indexes, and values outside Marrow
scalar envelopes raise `run.type`.

JSON text is bounded before and after parsing by fixed byte, depth, node, and
string-size caps. Integers must fit `int` and be JSON integers. The negative
zero integer spelling `-0` is rejected rather than normalized to `0`. A decimal
number is ingested leniently and canonicalized, so a trailing-zero fraction or
leading zero (`9.50`, `9.0`) reads as its one stored value (`9.5`, `9`); it must
still fit the decimal envelope, and exponent spellings do not become decimal
values.

## `std::csv`

CSV helpers read a narrow RFC 4180 subset from text and return scalar cells:

```mw
std::csv::row(cells: sequence[string]): string
std::csv::rowCount(text: string): int
std::csv::hasColumn(text: string, column: string): bool
std::csv::string(text: string, row: int, column: string): maybe string
std::csv::int(text: string, row: int, column: string): maybe int
std::csv::decimal(text: string, row: int, column: string): maybe decimal
std::csv::bool(text: string, row: int, column: string): maybe bool
```

`row` renders one RFC 4180 record with no trailing newline, the exact inverse of
the reader: it quotes a cell containing a comma, double-quote, CR, or LF and
doubles internal double-quotes. An empty sequence and a one-element sequence
holding an empty cell both render as an empty string, matching the reader.

The first row is the required header. Data rows are zero-based after the header.
Missing rows, missing columns, and empty cells are absent. Duplicate or empty
headers, ragged rows, malformed quotes, CR not followed by LF, and oversized
input raise `run.type`. Unquoted whitespace is preserved. Quoted fields may
contain commas, newlines, and escaped quotes as `""`. Like JSON, a decimal cell
is ingested leniently and canonicalized, so `9.50` reads as `9.5`.

## `std::id` And `std::random`

ID helpers return ordinary strings:

```mw
std::id::slug(text: string): string
std::id::stableUuid(seed: string): string
```

`slug` is ASCII-only: it lowercases `A-Z`, keeps `a-z` and `0-9`, collapses
other runs to one `-`, and trims hyphens. `stableUuid` hashes the seed and
returns a deterministic RFC 4122 version-4 UUID string. It does not create an
`Id(^store)` and is unrelated to `nextId`.

Deterministic random helpers are pure PRFs over `(seed, step)`:

```mw
std::random::int(seed: string, step: int, min: int, max: int): int
std::random::bool(seed: string, step: int): bool
std::random::decimal(seed: string, step: int): decimal
```

`step` must be non-negative and `min <= max`. `int` uses rejection sampling over
a SHA-256-derived `u128`; `decimal` returns an exact canonical decimal in
`[0, 1)` with up to 18 fractional digits. No helper reads host entropy.

## `std::context`, `std::audit`, And `std::error`

Run context is host-provided request metadata:

```mw
std::context::actor(): maybe string
std::context::requestId(): maybe string
std::context::idempotencyKey(): maybe string
```

A run without context capability raises `run.capability`. A provided context
with a missing field returns absence.

Audit helpers are pure compact JSON string builders; they do not write logs or
saved data:

```mw
std::audit::event(action: string, actor: string, subject: string): string
std::audit::change(field: string, before: string, after: string): string
```

Error helpers read the existing `Error` resource shape:

```mw
std::error::code(err: Error): string
std::error::message(err: Error): string
std::error::hasCode(err: Error, code: string): bool
```

`Error.code` uses the existing error-code grammar. In the current type lattice
`ErrorCode` stores as `string`, so these helpers return and compare strings
without weakening the `Error(...)` and `ErrorCode(...)` validation rules.
`hasCode` raises `run.type` when `code` is not valid error-code text.

## `std::matrix`

Matrices are canonical strings, not a distinct value type:

```mw
std::matrix::parse(text: string): string
std::matrix::identity(size: int): string
std::matrix::rows(matrix: string): int
std::matrix::cols(matrix: string): int
std::matrix::get(matrix: string, row: int, col: int): decimal
std::matrix::add(a: string, b: string): string
std::matrix::multiply(a: string, b: string): string
std::matrix::transpose(matrix: string): string
```

Canonical matrix text is bracketed rows separated by `;`, columns separated by
`,`, and canonical decimal cells, for example `[1,2;3.5,4]`. `parse` accepts
spaces around cells and separators and returns canonical text. Each decimal cell
is ingested leniently and canonicalized, so `9.50` reads as `9.5`, consistent
with `std::json` and `std::csv`. Rows and columns
are zero-based. Non-rectangular or malformed text, invalid indexes, incompatible
dimensions, and configured byte/dimension/cell/operation-limit violations raise
`run.type`. Addition and multiplication use exact Marrow decimal arithmetic and
raise the existing decimal overflow fault when an arithmetic result leaves the
decimal envelope.

## `std::assert` And Testing

Testing helpers work in ordinary `.mw` functions:

```mw
std::assert::isTrue(condition: bool)
std::assert::isFalse(condition: bool)
std::assert::equal(actual: T, expected: T)
std::assert::absent(path)
std::assert::fail(message: string)
```

`equal` accepts scalar values of the same type and fails with
`expected X, got Y`. It does not compare sequences, resources, trees,
identities, enums, or errors. Other boolean and ordering assertions remain
normal Marrow expressions:

```mw
std::assert::isTrue(a < b)
```

`absent(path)` is the testing counterpart to `exists(path)`. It does not hide
schema or decoding errors.

`marrow test` runs every `pub fn` with no parameters in a test file as a test;
other functions are helpers. Test files are selected by the project's `tests`
paths in `marrow.json`. A test file is named from its `tests`-relative path and
needs no `module` declaration; if it declares one, the name must match that
path-derived name, just as a source file's `module` must match its path. Each
test runs against a fresh in-memory store, so tests are independent and never
touch saved data. `marrow test` reports failures as typed `Error` values with
source locations.

## `std::log`

Logging writes to host-configured sinks:

```mw
std::log::info(message: string)
std::log::warn(message: string)
std::log::error(err: Error)
```

Application audit data is saved explicitly as resources when it must be
queryable.
