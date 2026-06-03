# Lane 5: Resource Store Surface

Lane 5 owns the public resource/store surface for Marrow v0.1: split resource
shape from store identity, store-owned indexes, canonical `Id(^store)` identity
types, and the checker/schema/docs contract that exposes those facts.

Status: complete for the Marrow v0.1 public resource/store surface.

## Durable Contract

- Split `resource` and `store` declarations are the semantic model.
- `store ^books(id: int): Book` owns durable identity, keys, and indexes.
- The concise `resource Book at ^books(id: int)` form is source sugar for the
  same split model.
- `Id(^store)` is the canonical identity type. Resource-name identities such as
  `Book::Id` exist only as rejection fixtures, not accepted surface.
- Index arguments may name store keys or top-level fields only. Dotted nested
  fields and bare leaves nested inside unkeyed groups are schema errors.
- A resource name resolves through the module-aware checked resolver. Tooling
  surfaces that render resource-name resolution must not perform project-wide
  resource-name fallback.

## Maintained Invariants

- Any tool or runtime surface that renders resource-name resolution uses the
  checked, module-aware resolver; no production helper performs a project-wide
  resource-name search.
- Checker annotations, checked facts, constructor fields, local member reads,
  saved member reads, schema-query defaults, and runtime constructor backstops
  share the same resource-first resolution contract.
- A same-module resource owns its annotation before enum fallback or enum
  visibility diagnostics are considered.
- Foreign bare resource names are unresolved from the caller's module, and
  duplicate visible bare resource names are ambiguous rather than first-matched.
- Canonical language docs, schema tests, and checker behavior all reject dotted
  nested index arguments for v0.1.
- Schema tests assert field classification through production outputs or
  production-owned helper APIs.
- Touched checker/runtime code is split around explicit context structs and
  focused helpers instead of clippy shape suppressions.

## Audit Criteria

- No production path resolves resources by project-wide first match.
- No test or CLI path preserves bare-resource fallback behavior.
- No duplicate semantic classifier owns plain-field, resource, store, or index
  meaning outside production APIs.
- Canonical docs, schema tests, and checker behavior agree on top-level-only
  index arguments.
- Touched Rust does not rely on clippy shape suppressions, oversized dispatcher
  functions, compatibility shims, or comments that stand in for decomposition.

## Stable Boundaries

- Lane 6 enum and presence semantics are unchanged except where type annotations
  must obey this lane's resource-first contract.
- Lane 7 store internals, physical keys, backup/restore, and persistence engine
  behavior are unchanged.
- Raw/debug/admin inspection may expose stored index trees, but production
  resource resolution and typed writes remain governed by checked store facts.

## Feature-Surface Audit Note

Lane 5 stays complete unless a later audit finds a concrete resource/store
surface regression. Do not reopen this lane just because Lane 10 deletes,
demotes, or rebuilds `marrow explain`; Lane 5 owns the resource/store resolver
contract, not that command as a product feature.

Reopen Lane 5 only for a narrow fix if a surviving production surface accepts
resource-name identity such as `Book::Id`, dotted or nested index arguments,
project-wide resource-name fallback, raw path identity as store identity, or a
duplicate resource/store classifier outside the production APIs.
