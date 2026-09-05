# Execution limits

Every bound on a Marrow program is a fixed number. A program that crosses one
gets a diagnostic at the construct that crossed it, or a fault at the
instruction that did.

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

| Group | Limit | Value | Diagnostic |
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
| Runtime | Text value | 64 KiB | `run.text_limit` |
| Runtime | Elements in a list or map | 65,536 | `run.collection_limit` |
| Runtime | Size of a list or map | 1 MiB | `run.collection_limit` |

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
calls. The text limit applies to a text built by concatenation or `join`. The
collection limits apply whenever a list or map grows: `append`, map insertion,
`split`, and `lines`.

The compiled program size is checked as function bodies are compiled. Once the
bodies compiled so far alone exceed it, checking stops at that body and reports
`cli.compiler_resource_limit`; diagnostics found before the stop are not reported
and reappear once the program fits.

These limits are fixed.
