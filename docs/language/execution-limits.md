# Execution limits

Marrow uses fixed bounds for source, compiled programs, and runtime values.
Source limits produce diagnostics; runtime operations produce faults at their
instructions. Argument admission can refuse a request before execution.

A loop with no bound of its own:

```mw
module docs::limits::spin

pub fn spin(): int {
    var n = 0
    while true {
        n += 1
    }
    return n
}
```

```text
$ marrow run docs.limits.spin.spin
run.budget at 6:9
```

`while` has no iteration limit. The invocation's instruction budget is shared
across its whole call tree, so a loop that never terminates exhausts it and
faults with `run.budget` at the instruction that ran out. The fault carries a
source position and ends the invocation.

A declaration that is too wide is a source diagnostic:

```text
$ marrow check .
src/main.mw:3:1: check.resource_limit: a function declares 17 parameters; the fixed limit is 16
```

The diagnostic points at the declaration. When no single construct is at
fault, because a count across the whole program or the compiled program's size
crossed a bound, `marrow check` reports `cli.compiler_resource_limit` without
a source position.

## Limits

| Group | Limit | Value | Failure |
|---|---|---:|---|
| Source | Nesting of expressions and blocks | 256 levels | `check.nesting_limit` |
| Source | Diagnostics retained by one run | 4096, or 1 MiB of text | `cli.compiler_resource_limit` (`fmt.diagnostic_limit` under `marrow fmt`) |
| Source | Traversal bound `at most N` | 65,536 | `check.type` |
| Declarations | Store roots in a project | 4096 | `cli.compiler_resource_limit` |
| Declarations | Fields in one resource | 4096 | `check.resource_limit` |
| Declarations | Key components of a root or branch | 8 | `check.resource_limit` |
| Declarations | Indexes on one root | 8 | `check.type` |
| Declarations | Member nesting (groups and branches) | 16 levels | `check.resource_limit` |
| Declarations | Value nesting in a stored field | 32 levels | `check.resource_limit` |
| Declarations | Leaves in a stored struct value | 64 | `check.resource_limit` |
| Declarations | Members of one enum | 256 | `check.resource_limit` |
| Declarations | Payload fields of one enum member | 64 | `check.resource_limit` |
| Declarations | Parameters of one function | 16 | `check.resource_limit` |
| Declarations | Exported functions in a project | 256 | `cli.compiler_resource_limit` |
| Declarations | Tests in a project | 256 | `cli.compiler_resource_limit` |
| Declarations | Compiled program size | 512 KiB | `cli.compiler_resource_limit` |
| Runtime | Instruction budget per invocation | 2^26 | `run.budget` |
| Runtime | Call depth | 64 | `run.call_depth` |
| Runtime | Constructed text | 64 KiB | `run.text_limit` |
| Collections | List elements or Map pairs | 65,536 | [Collection limits](#collection-limits) |
| Collections | Aggregate structural size | 1 MiB | [Collection limits](#collection-limits) |

The source limits apply while a file is parsed and checked. The declaration
limits apply at a `resource`, `store`, `enum`, or `fn` header, or across the
whole project for a count of roots, exports, or tests. The runtime limits
apply to one invocation of one export.

Source nesting counts every brace and bracket that encloses a construct: a
block inside a block, a parenthesis inside a parenthesis, an operand inside an
operator. Member nesting counts groups and branches under a resource; value
nesting counts structs and enums inside a stored field, with a scalar as level
one. A [traversal bound](traversal-and-indexes.md#bounded-durable-traversal)
above 65,536 is reported at the number in the `for` head.

Call depth counts active calls in one invocation. Recursion is a compile
error, so the depth limit is reached only by a very deep chain of distinct
calls. The text limit applies to text built by concatenation, `join`, or
`string(...)`, including conversion of interpolation holes. It counts UTF-8
bytes of the complete canonical text, including punctuation and hex expansion.
A conversion that would exceed the limit faults with `run.text_limit` at its
source expression before appending excess text. This is a result-length limit,
not a bound on total invocation memory or on aggregate CLI output.

The compiled program size is checked as function bodies are compiled and again
when the image is encoded. Once it is crossed, checking stops at that bound and
reports `cli.compiler_resource_limit`; other diagnostics the program carries are
not reported until it fits.

The runner reads at most 524,289 image bytes before verification. An image larger
than 512 KiB is refused with `image.envelope`; the extra byte distinguishes an
oversized input from an image exactly at the limit. This bounds input bytes read,
not allocation capacity or elapsed time. A stream that supplies neither EOF nor
enough bytes to reach this read limit can still block.

## Collection limits

A List has at most 65,536 elements and a Map at most 65,536 pairs. Each has at
most 1,048,576 aggregate structural bytes. Both limits include equality.
A List's aggregate is the sum of its elements' structural sizes; a Map's is
the sum of its keys' and values' sizes. Each nested collection also satisfies
its own limits.

Structural size uses the following measure:

| Value | Structural bytes |
|---|---|
| `int`, `date` | 8 |
| `instant`, `duration` | 16 |
| `bool` | 1 |
| `string`, `bytes` | UTF-8 byte length or byte length, respectively |
| Absent optional | 1 |
| Present optional | 1 plus the contained value's size |
| Struct or resource value | 1 plus the field sizes; an absent sparse field contributes 1 |
| Enum | 1 plus the payload values' sizes |
| Nested List or Map | 1 plus its aggregate size |
| `Id(^root)` | 1 plus its key scalars' sizes |

Keys use the same scalar sizes. A collection's own aggregate excludes its
one-byte framing contribution to a containing value. Allocation capacity,
object and allocator overhead, decoded JSON, and frame copies lie outside this
metric. The [wire frame limit](../tools/typescript-client.md#type-projection)
applies separately to the encoded body.

The runner checks decoded List and Map arguments, including collections nested
inside other values. An excess produces `runner.arg_mismatch` before the export
runs. VM `append`, Map insertion or value replacement, `split`, `lines`, and
durable or index traversal report `run.collection_limit` at the operation when
the resulting collection exceeds a limit.

A post-dispatch reply rejected by the Rust companion client's typed decoder for
exceeding these limits is an outcome-unknown result with `ReplyDecode` as its
cause.
