# General-purpose language

This page is future direction. The current language is useful for its prototype
durable model but does not yet provide the planned general-purpose floor.

## Goal

A program with no durable declaration should still be a complete Marrow
program. It should be able to define reusable modules and ordinary data types,
transform local collections, use packages, call explicit host facilities,
compile to the same image format, and run on the same VM as a durable program.

The v0.1 direction includes algebraic data types and exhaustive patterns, real
parametric functions and user types, lexical closures, generic local
collections, eager higher-order helpers, lexical iteration, `Option`, `Result`,
exact decimal arithmetic, and a small closed constraint vocabulary. Struct and
algebraic-data-type values are dense. Expected parsing, validation, host, and
business failures use ordinary typed values; verifier, integrity, authority,
budget, arithmetic, and commit faults form a separate closed,
source-uncatchable beta channel. Input-derived arithmetic uses ordinary checked
operations when the program needs to report overflow or division failure as a
typed result; faulting operators remain available for cases where failure is a
program fault.

## Constraints

- Generic bodies are checked parametrically rather than expanded into built-in
  overload tables.
- Function types preserve one finite inferred effect bound for higher-order
  arguments without requiring ordinary source to repeat effect rows.
- Runtime closure values cannot smuggle mutable, durable, host, transaction, or
  authority handles.
- Persistent local collections require capture-eligible stored components;
  affine handles may be threaded as standalone generic state but are not
  collection elements or values in the beta.
- By-value affine calls consume their binding and must return an explicit
  successor to continue; method spelling introduces no implicit borrow or
  builtin-only receiver rule.
- Evaluation order, faults, recursion, specialization, allocation, and aggregate
  materialization have documented bounds.
- A completed non-unit expression cannot be discarded in statement position,
  bound by `let _ = expression`, or left in a named dead binding. This ordinary
  rule applies equally to `Result`, collection results, durable mutation
  outcomes, and prune progress; wildcard match subpatterns and wildcard
  function parameters remain ordinary pattern vocabulary.
- Storeless checking, compilation, tests, formatting, and editor facts never
  initialize or inspect a store.
- Storeless host access uses bounded terminal and pre-opened UTF-8 text handles;
  importing a package supplies no ambient filesystem, network, clock, entropy,
  process, or compiler authority.

Higher-rank and higher-kinded types, polymorphic recursion, open-world
instances, trait objects, macros, implicit coercions, and first-class lazy
iterators are not required for the beta.

## Evidence target

A useful command-line graph-reporting program must exercise these features
through project initialization, check, format, source tests, build, run, exact
Git acquisition, offline rebuild, the compiler, image verifier, VM, formatter,
and LSP without a store or feature-specific escape hatch. Its generic graph
types, parser, collection transforms, closures, and error values must be
expressible by application and source-library code rather than privileged
built-ins.
