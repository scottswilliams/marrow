# Enums

Behavior designed but not yet implemented.

An `enum` is a named, fixed set of values, declared in source — the user-defined
generalization of `bool`. Its members are the values themselves rather than
fields holding values.

```
enum Status
    active
    archived
    banned
```

A member is written `Status::archived` and used like any scalar: compared with
`==`, stored in a field (`state: Status`), used as a key, and matched. It stores
compactly and renders as its member name. `match` over an enum is exhaustive —
every member must be handled.

Members may nest into a tree; a value is then the selected member, `is` tests a
subtree (`pet is Cat::tiger`), and a member marked `abstract` is a category that
cannot be selected directly. A flat enum is a one-level tree.

A set whose members are managed at runtime is not an enum — it is a saved
resource referenced by a field. An enum is fixed in source so its `match` can be
exhaustive.

`enum` takes `pub` like any declaration.
