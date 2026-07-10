# Execution Limits

The current parser and runtime enforce fixed limits that are observable by
Marrow programs and source files.

## Core Limits

| Limit | Value | Result when exceeded |
|---|---:|---|
| Source nesting | 256 levels | Located check diagnostic |
| Function-call nesting | 256 active calls | Typed runtime depth error |
| One transaction staged-write budget | 64 MiB | Transaction-too-large error and rollback |

Source nesting includes indentation blocks and expression nesting such as
parentheses, unary expressions, and binary operands. The entry function is call
depth 1; attempting depth 257 fails before native stack overflow.

The transaction limit meters the estimated buffered write footprint, including
per-record and per-cell overhead as well as variable path, key, and value bytes.
It is not a raw serialized-data byte count. Nested transactions share the
outer transaction and its budget.

## Library Input Limits

| Module | Limits |
|---|---|
| `std::json` parsing and accessors | 1 MiB input, depth 64, 10,000 nodes, 65,536 bytes per string |
| `std::csv` parsing and accessors | 1 MiB input, 10,000 rows, 256 columns, 65,536 bytes per cell |
| `std::matrix` | 1 MiB text, dimension 64, 4,096 cells, 100,000 arithmetic operations |

The JSON row covers `valid` and the pointer accessors, not `stringLit` or
`stringArray`. The CSV row covers `rowCount`, `hasColumn`, and the typed readers,
not the `row` builder. Builder output is constrained by available memory rather
than these parser bounds. Bounded accessors and matrix functions raise a runtime
type error when their limit is exceeded. `std::json::valid` instead returns
`false` for malformed or over-limit JSON. These bounds apply per call.

## Unbounded Work

`while` and `for` do not have a step or fuel limit. A nonterminating `while`
continues indefinitely, subject only to external process control. Traversal of
a durable collection visits every matching stored entry unless the body exits.

Collection size, durable-tree depth, and the total number of writes in a run
have no single language-wide count limit. Available memory, store capacity,
host capabilities, and the transaction budget still constrain an execution.
