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
and a small closed constraint vocabulary.

## Constraints

- Generic bodies are checked parametrically rather than expanded into built-in
  overload tables.
- Function types preserve the effects of higher-order arguments.
- Runtime closure values cannot smuggle mutable, durable, host, transaction, or
  authority handles.
- Evaluation order, faults, recursion, specialization, allocation, and aggregate
  materialization have documented bounds.
- Storeless checking, compilation, tests, formatting, and editor facts never
  initialize or inspect a store.

Higher-rank and higher-kinded types, polymorphic recursion, open-world
instances, trait objects, macros, implicit coercions, and first-class lazy
iterators are not required for the beta.

## Evidence target

A useful command-line graph-reporting program must exercise these features
through the package resolver, compiler, image verifier, VM, formatter, tests,
and LSP without a store or feature-specific escape hatch.
