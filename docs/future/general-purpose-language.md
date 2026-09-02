# General-purpose language

A program with no durable declaration is a complete Marrow program. It compiles
to the same image and runs on the same VM as a durable program.

## Today

The storeless language is current and defined in the
[reference](../language/README.md): modules and functions, generic functions
and types with `supports equality` and `supports order`, structs, enums with
exhaustive `match`, `Option` and `Result`, lists and maps, `date`, `instant`,
and `duration`, source tests, formatting, and editor facts. Every value copies
by value. Source names no memory, store handle, or transaction object.

Integer arithmetic faults on overflow with `run.overflow`. The `checked` form
names an arm for each way the arithmetic can fail
([control flow](../language/control-flow.md#checked-arithmetic)). Faults are
not catchable ([errors and transactions](../language/errors-and-transactions.md)).
Checking, compiling, testing, and formatting a storeless project open no store.

## Direction

Host input and output. A storeless program reads and writes text through
bounded terminal and pre-opened UTF-8 text handles that the host supplies.
Temporal values carry no clock, so the current time is one such host input.
Importing a package supplies no filesystem, network, clock, entropy, or
process access.

Packages, with exact dependency edges ([packages](packages.md)).

Enum members that group other members, with a membership test over the group.
The checker reports the grouping form as unsupported today.

A `decimal` type. There is no floating-point type.

Closures and a set type are not planned. Traits, dynamic dispatch, higher-rank
and higher-kinded types, macros, implicit coercions, and lazy iterators are
outside the language.

## Evidence

Graph Report (`fixtures/v01/conformance/graph_report`) is the storeless
acceptance program ([local applications](local-applications.md)). It becomes
this page's evidence when it also imports one Git package and reads its input
through a host handle, and passes init, check, format, test, run, and an
offline rebuild without a store.
