# General-purpose language

A program with no durable declaration is a complete Marrow program. It compiles
to the same image and runs on the same VM as a durable program.

## Today

The storeless language is current and defined in the
[reference](../language/README.md): modules and functions, generic functions
and types with `supports equality` and `supports order`, structs, enums with
exhaustive `match`, `Option` and `Result`, lists and maps, `date`, `instant`,
and `duration`, source tests, formatting, and editor facts. Every value copies
by value. Source names no memory, store handle, or transaction object. The
toolchain supplies no `std::` modules, and a project-declared `std::` path is
ordinary project code ([builtins](../language/builtins.md#no-standard-library)).

Integer arithmetic faults on overflow with `run.overflow`. The `checked` form
names an arm for each way the arithmetic can fail
([control flow](../language/control-flow.md#checked-arithmetic)). Faults are
not catchable ([errors and transactions](../language/errors-and-transactions.md)).
Checking, compiling, testing, and formatting a storeless project open no store.

The terminal exposes bounded text input and output through an adapter
around an ordinary string-taking, string-returning export. Graph Report needs
no in-language host-call system to read input and emit its report. The adapter
bounds input before invocation and output at the process boundary; it adds no
ambient access to the language.

## Direction

In-language terminal or pre-opened text handles, clocks and generalized Rust
host bindings are deferred. If introduced later, host effects precede durable
access across the call graph. Importing source supplies no filesystem, network,
clock, entropy or process access.

Source reuse is exercised through maintained programs; the package direction
uses exact dependency edges ([packages](packages.md)). Portable library
behavior belongs in ordinary Marrow source, compiled and verified with its
caller, using the same generics, value types and runtime as application code;
a helper is extracted when maintained programs share real behavior, and an
intrinsic is justified only when source cannot express an operation portably or
within measured bounds. The beta needs source reuse, not a standard-library
portfolio, toolchain-pinned package lineage, combinator framework or new
privileged namespace; broader library organization waits until actual callers
show what it must contain. Decimal arithmetic and
enum grouping are deferred until a maintained program demonstrates a need and
an implementation can be compared with a simpler alternative. Their parsed
forms are unsupported today; neither is a beta prerequisite. There is no
floating-point type.

A set type is not planned. Closures and indirect-call demand are deferred
([path effects and authority](path-effects-and-authority.md)). Traits, dynamic
dispatch, higher-rank and higher-kinded types, macros, implicit coercions, and
lazy iterators are outside the language.

## Evidence

Graph Report (`fixtures/v01/conformance/graph_report`) takes and returns strings
and reuses one local source dependency. Production-path tests exercise its
terminal adapter's bounded input and output, including multiline standard input,
without a store. Exact Git acquisition and a standard-library package are not
required for this evidence.

A source helper is proven when two maintained callers reuse it with fewer
duplicated rules and unchanged behavior, compiling and testing through ordinary
tooling with no privileged initialization or host authority.

Each widening requires production compiler, verifier, runtime and client
agreement. Nominal-bearing public aggregates and durable values remain refused
until their constraints survive those boundaries; general nominal transport is
outside the beta target. New optional parameter placements, omitted/default
arguments, and wider resource or collection combinations are deferred unless a maintained caller requires them;
each supported combination must have the same meaning at every boundary.
