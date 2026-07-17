# Standard Library

The beta line has no `std::` standard library. Qualified standard-library calls
such as `std::text::trim(value)` are not resolved by the current checker; a
`std::` path reports `check.type` until a standard-library owner is refounded.

The current pure floor is the set of built-ins available without `use`:
presence checks, the text floor (`isEmpty`, `contains`, `trim`, `split`,
`lines`, `join`), the finite collections `List<T>` and `Map<K, V>` with their
operations, and the error constructors. See [Built-ins](builtins.md) for the
complete current set, and [Types and values](types-and-values.md#lists-and-maps)
for collection value semantics.

A broader standard library (text, bytes, hashing, mathematics, clock, JSON/CSV,
and host capabilities) is future direction recorded under
[`docs/future/`](../future/); it is not current syntax or a guarantee.
