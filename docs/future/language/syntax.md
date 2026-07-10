# Syntax

> **Status: retired proposal.** This page is non-authoritative and may conflict
> with the current vision. Do not implement it without an accepted target
> contract; see [Retired design notes](../README.md).

Future counterpart of [`../../language/syntax.md`](../../language/syntax.md).

The v0.1 parser reports typed diagnostics for several recognizable but
unsupported forms. These forms are not accepted syntax and have no formatter
round trip today.

## Unsupported forms

The reserved unsupported family includes:

- bracket collection literals;
- `finally`;
- loop labels;
- parameter defaults;
- parameter modes;
- quoted field segments;
- type aliases;
- user-defined generics.

Each form needs a source contract before it can ship. Until then, a diagnostic
for one of these forms means the spelling is rejected, not partially supported.

## Collection literals

Bracket collection literals belong with the future map/set collection family.
The future contract covers empty-literal typing, element type inference,
ordering, duplicate-key behavior, and the boundary between local collection
values and saved keyed layers.

## Control-flow extensions

`finally` and loop labels are deferred control-flow extensions. A future
`finally` states whether cleanup runs on `return`, `break`, `continue`, and
throw, plus how cleanup errors interact with transaction rollback. A future loop
label names which loop a labeled `break` or `continue` targets and whether
labels apply to saved-data traversal loops.

## Parameter and type extensions

Parameter defaults, parameter modes, type aliases, and user-defined generics are
deferred declaration and type-system extensions. Their future contract must
state public ABI effects, generated-client rendering, type identity, diagnostic
rendering, and saved-schema compatibility.

## Quoted field segments

Quoted field segments are deferred. A future spelling covers escaping, catalog
path identity, generated-client names, and interaction with parser-reserved
words. v0.1 field and member names remain ordinary identifiers.
