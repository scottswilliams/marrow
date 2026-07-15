# General-purpose language

This page is future direction. The current language is useful for its prototype
durable model but does not yet provide the planned general-purpose floor.

## Goal

A program with no durable declaration should still be a complete Marrow
program. It should be able to define reusable modules and ordinary data types,
transform local collections, use packages, call explicit host facilities,
compile to the same image format, and run on the same VM as a durable program.

The beta floor is an ordinary storeless language: algebraic data types with
exhaustive patterns, real rank-1 parametric functions and types, generic local
collections, modules, source tests, formatting, editor facts, `Option`,
`Result`, and narrow temporal value types. Struct and algebraic-data-type
values are dense. Expected parsing, validation, host, and business failures use
ordinary typed values; verifier, integrity, authority, budget, arithmetic, and
commit faults form a separate closed, source-uncatchable beta channel.

## Constraints

- Values have ordinary high-level value semantics. Memory management is hidden
  from source: there is no ownership or borrow annotation, no consume-and-return
  successor obligation, and no first-class store, host, transaction, or
  authority handle exposed as a source value.
- Parametric bodies are checked once and monomorphized through a single lowering
  rather than expanded into built-in overload tables. Constraints are limited to
  the closed set of equality and ordering; there are no traits, dictionaries,
  dynamic dispatch, higher-rank types, or higher-kinded types.
- Generic local collections provide finite ordered lists and maps. A set type is
  added only if a maintained program is materially worse without it.
- Narrow temporal value types cover dates, instants, and durations. They carry
  no ambient clock; the current time is an explicit host input rather than an
  operation available to any pure function.
- Default integer arithmetic faults on overflow. An adjacent explicit checked
  form reports overflow and division failure as an ordinary typed result for
  input-derived arithmetic that must handle it. The beta floor has no decimal or
  floating-point type.
- The current enum is a flat closed set of members, each bare or carrying a
  dense typed payload, matched exhaustively with no wildcard arm. Hierarchical
  enums — `category` members that group descendants, qualified arms such as
  `cat::tiger`, and the `is` subtree-membership operator — are a deferred slice;
  the parser accepts the `category` and nesting syntax, but the checker rejects
  it until that slice lands.
- The beta rejects direct and mutual function recursion and recursive nominal
  value layouts. Both remain deferred; recursive durable relationships use keys
  in a finite branch topology rather than recursive value expansion.
- Evaluation order, faults, specialization, allocation, and aggregate
  materialization have documented bounds.
- Storeless checking, compilation, tests, formatting, and editor facts never
  initialize or inspect a store.
- Storeless host access uses bounded terminal and pre-opened UTF-8 text handles;
  importing a package supplies no ambient filesystem, network, clock, entropy,
  process, or compiler authority.

Closures are deferred until a maintained program is materially worse without
them; they are an evidence-driven addition, not a prerequisite for the floor.
Higher-rank and higher-kinded types, polymorphic recursion, open-world
instances, trait objects, macros, implicit coercions, and first-class lazy
iterators are not part of the beta.

## Evidence target

A useful command-line graph-reporting program must exercise these features
through project initialization, check, format, source tests, build, run, exact
Git acquisition, offline rebuild, the compiler, image verifier, VM, formatter,
and LSP without a store or feature-specific escape hatch. Its generic graph
types, parser, collection transforms, and error values must be expressible by
application and source-library code rather than privileged built-ins.

This page states direction. [Project status](../status.md) separates current,
legacy, and future behavior; [durable programming](durable-programming.md)
records the durable direction that reuses this same floor.
