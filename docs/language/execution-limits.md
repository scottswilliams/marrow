# Execution Limits

Marrow enforces fixed limits at three layers, each with a distinct owner: the
parser bounds source nesting, the independent verifier bounds the program image
before it allocates, and the virtual machine bounds one invocation's dynamic work.
None of these limits is configurable, and no runner, environment variable, or
caller can raise them.

## Source Nesting

| Limit | Value | Result when exceeded |
|---|---:|---|
| Source nesting | 256 levels | `check.nesting_limit` at the offending span |

Source nesting counts indentation blocks and expression nesting such as
parentheses, unary expressions, and binary operands. The parser fails closed
before native stack overflow.

## Program-Image Bounds

The verifier rechecks every representational bound against the received bytes
before it allocates, so a hostile or malformed image cannot drive unbounded work.
A violation is an `image.table` or `image.function` rejection.

| Limit | Value |
|---|---:|
| Whole image | 256 KiB |
| String-pool entries / bytes per entry | 1024 / 4 KiB |
| Constant-pool entries | 1024 |
| Functions / params per function | 64 / 16 |
| Locals per frame | 256 |
| Code bytes per function | 64 KiB |
| Operand-stack depth | 256 |

The operand-stack depth is computed and sealed by the verifier, never read from
the image. These bounds size the current subset; widening any of them is a later
decision recorded with its own coverage.

## Per-Invocation Runtime Limits

The virtual machine owns the dynamic limits of one invocation. They are private
constants with no override path.

| Limit | Value | Result when exceeded |
|---|---:|---|
| Instruction budget | 67,108,864 (2^26) steps | `run.budget` |
| Call depth | 64 active calls | `run.call_depth` |
| Text-concatenation result | 64 KiB | `run.text_limit` |

The instruction budget is shared across the whole call tree of one invocation, so
total work stays bounded regardless of loop or call structure. A `while` or `for`
loop has no separate iteration limit, but a non-terminating loop still exhausts
the instruction budget and faults with `run.budget` rather than running forever.
Static recursion is rejected at verification, so the call-depth limit guards a
pathologically deep non-recursive call chain. Each faulting instruction maps to
its source span, and none of these faults is catchable inside the program.
