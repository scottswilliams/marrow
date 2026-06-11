# Builtins

Future counterpart of [`../../language/builtins.md`](../../language/builtins.md).

## Set Membership

`insert(path)` populates a `set[K]` member. A set member carries no value, so
there is no right-hand side to assign; `insert` is the populate verb, as
`append` is for a sequence. The existing `delete path` and `exists(path)` clear
and test a member. See the collection spellings in
[`resources-and-storage.md`](resources-and-storage.md).
