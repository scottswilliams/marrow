# Builtins

Future counterpart of [`../../language/builtins.md`](../../language/builtins.md).

## Set Membership

`insert(path)` populates a `set[K]` member. A set member carries no value, so
there is no right-hand side to assign; `insert` is the populate verb, as
`append` is for a sequence. The existing `delete path` and `exists(path)` clear
and test a member. See the collection spellings in
[`resources-and-storage.md`](resources-and-storage.md).

## Conversions

Conversion calls are explicit, checked conversions. The checker accepts a
conversion only when the source type is statically supported for the target; an
unsupported static conversion is a check error rather than a delayed runtime
guess. `unknown` remains the dynamic boundary: converting an `unknown` value is
checked at runtime against the same supported source shapes.

Supported conversions are the useful boundary conversions:

| Target | Statically supported sources |
|---|---|
| `int` | `int`, canonical integer `string`, integral `decimal` in range, `unknown` |
| `decimal` | `decimal`, `int`, canonical decimal `string`, `unknown` |
| `bool` | `bool`, canonical boolean `string`, canonical `0` or `1` integer, `unknown` |
| `string` | `string`, `int`, `decimal`, `bool`, `bytes`, `ErrorCode`, `date`, `instant`, `duration`, `unknown` |
| `bytes` | `bytes`, UTF-8 `string`, `unknown` |
| `ErrorCode` | `ErrorCode`, `string`, `unknown` |
| `date` | `date`, canonical date `string`, `unknown` |
| `instant` | `instant`, canonical instant `string`, `unknown` |
| `duration` | `duration`, canonical duration `string`, `unknown` |

String conversions produce canonical Marrow text for typed values. Converting
`bytes` to `string` decodes UTF-8 and faults when the bytes are not valid text;
converting `string` to `bytes` encodes UTF-8. Temporal conversion calls accept
canonical Marrow text only. Use the `std::clock` parse helpers for permissive
user and host boundary text.
