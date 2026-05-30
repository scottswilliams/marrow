# Enum hierarchies

Behavior designed but not yet implemented. Flat enums are live; see
[Enums](../../language/enums.md). This page covers the deferred hierarchy layer.

Members may nest into a tree. A flat enum is a one-level tree, so there is one
construct, not two.

```mw
enum Cat
    category tiger
        bengal
        siberian
    housecat
```

A value is a single selected member, stored as one ordinal exactly as a flat
enum value is. The hierarchy lives in the schema, not in the value, so flat and
nested enums share one storage model and existing data needs no migration.

A member marked `category` groups its descendants and cannot be selected as a
value; only its concrete members can. Above, `Cat::tiger` is unselectable, while
`Cat::tiger::bengal` and `Cat::housecat` are values. Members are selectable by
default, and `category` opts a node out.

`is` tests membership in a subtree: `pet is Cat::tiger` holds for any value at
or under `tiger`, such as `Cat::tiger::bengal`, and for a leaf it is exact. It
complements `==`, which is exact nominal equality.

`match` over a nested enum is exhaustive over the selectable members; an arm may
name a category to cover its whole subtree.
