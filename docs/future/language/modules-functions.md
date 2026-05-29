# Modules And Functions

Future counterpart of
[`../../language/modules-functions.md`](../../language/modules-functions.md). The
visibility change below is designed but not yet implemented.

## Visibility

`pub` is one uniform, enforced marker on every top-level declaration: `pub fn`,
`pub resource`, `pub enum`, `pub const`. A declaration is private to its module
unless marked `pub`, and referencing a non-`pub` declaration from another module
is a checked error. For a saved resource, `pub` governs which modules may read or
write its `^` root — visibility is data ownership, not only type naming.
