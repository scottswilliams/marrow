# Enums

An `enum` is a named, fixed set of values, declared in source — the user-defined
generalization of `bool`. Its members are the values themselves rather than
fields holding values.

```mw
enum Status
    active
    archived
    banned
```

The body is an indented block of bare member names, one per line, in declaration
order. An enum needs at least one member, and a member is a plain name: it takes
no type and no key parameters.

A member is written `Status::archived`. It is a value of the enum type, so it can
be the type of a field (`state: Status`), a parameter, a `var`, or a `const`, and
it can be compared with `==`. Equality is nominal: an enum value equals only a
value of the same enum. Comparing an enum to a raw string, or to a member of a
different enum, is a type error, as is any arithmetic or ordering operator.

Nominal identity holds at every typed boundary, not only `==`. An enum-typed
parameter, return, field, `var`, or `const` accepts only that exact enum: passing
or assigning a different enum (or a raw scalar) where one enum is expected is a
compile error, and a `match` is type-checked against the scrutinee's enum.

An enum's identity is its owning module together with its name. A bare `Status`
resolves to the enclosing module's `Status` first, so two modules may each declare
a `Status` without colliding; they are distinct enums and never compare equal. To
name another module's enum, qualify it: `b::Status` as a type and `b::Status::open`
as a value. A qualified name always names exactly that module's enum, so an
annotation and a value spelled the same way denote the same enum. The qualifier
may be a `use`-imported short alias: after `use a::b`, both `b::Status` and
`b::Status::open` name module `a::b`'s enum, the same way the alias resolves a
call.

A value stores compactly as the ordinal of the selected member — its position in
declaration order, starting at zero. So a `state: Status` field set to
`Status::archived` stores the int `1`. At the language level the field reads back
as its member: a read of `state` is a `Status` value, equal to `Status::archived`
again. Raw inspection works on the stored bytes, so `marrow data get` shows the
ordinal `1`.

## Hierarchies

Members may nest into a tree. A flat enum is the degenerate one-level tree, so
there is one construct, not two.

```mw
enum Cat
    category tiger
        bengal
        siberian
    housecat
```

A member's nested members are the indented block beneath it. Names must be unique
among siblings, but the same name may appear under different parents:
`Cat::tiger::paw` and `Cat::lion::paw` do not collide.

A member is named by a path from the enum down to it: the full
`Cat::tiger::bengal` walks the tree one segment per level. A shorter `Cat::bengal`
works too, naming the member by its leaf name — but only when that name is unique
in the enum. A name shared by several parents (`paw` above) cannot be picked out
by the bare `Cat::paw`; the compiler rejects it and asks for the qualifying path
(`Cat::tiger::paw` or `Cat::lion::paw`). The full path always resolves, so a
duplicate name is fully usable — it just has to be written out.

A value is still a single selected member, stored as one ordinal exactly as a flat
enum value is. Ordinals are assigned in pre-order — a parent before its children,
in source order among siblings — so a flat enum keeps the same `0..n` ordinals it
always had. The hierarchy lives in the schema, not in the value, so flat and
nested enums share one storage model and existing data needs no migration.

A member marked `category` groups its descendants and cannot be selected as a
value; only its concrete members can. Above, `Cat::tiger` is a category and is
unselectable, while `Cat::tiger::bengal` and `Cat::housecat` are values.

A member with nested members **must** be marked `category`. A grouping node is never
a value — a value selects one of its concrete descendants, not the group — and a
`match` covers the leaves under it, not the node itself, so an unmarked parent would
be a value no arm could ever handle. A non-category member with children is a compile
error. Symmetrically, a `category` must have nested members: one with nothing under it
can never be selected or matched, so it too is rejected. The two rules pin the
invariant that a member is a category exactly when it has children, leaving every
non-category a concrete, selectable leaf.

`category` is a modifier, not a reserved word: it is recognized only as the lead of
an enum-member line, so it remains usable as an ordinary identifier elsewhere.

## The `is` operator

`is` tests membership in a subtree:

```mw
if pet is Cat::tiger
    groom(pet)
```

`pet is Cat::tiger` holds for any value at or under `tiger`, such as
`Cat::tiger::bengal`, and is `false` for a value outside it, such as
`Cat::housecat`. For a concrete-leaf right operand it is exact: `pet is
Cat::bengal` holds only for a `bengal`. So `is` complements `==`, which is always
exact nominal equality — `is` widens the test to a whole subtree, while `==` stays
a single member.

The left operand must be an enum value and the right a member of the same enum (a
concrete member or a category); the result is `bool`. The right operand is a member
path like any other, so a duplicated leaf is reached by its full path (`pet is
Cat::tiger::paw`) and a bare duplicated name is rejected the same way as in value
position. `is` is a reserved word, so it cannot be used as an identifier. It does
not chain: `a is X is Y` is a syntax error.

## Matching

`match` runs the arm for an enum value's member:

```mw
match order.state
    active
        reopen(order)
    archived
        ; leave it
    banned
        notify(order.owner)
```

The header is `match <value>`, where the value is enum-typed. Each arm is a member
path *relative* to the scrutinee enum — the scrutinee supplies the enum, so an arm
is `archived`, not `Status::archived` — followed by the indented block to run when
the value selects a member under it. An arm may be a bare leaf (`bengal`), a
qualified path (`tiger::bengal`), or a category (`tiger`, covering its whole
subtree).

Over a nested enum, an arm may name a category to cover its whole subtree, or a
qualified path to reach one leaf:

```mw
match pet
    tiger::bengal
        show(pet)
    tiger::siberian
        groom(pet)
    housecat
        feed(pet)
```

A bare arm name must be unambiguous, just as a bare `Enum::member` value must: a
name shared by several parents (`paw`) is rejected with the qualifying paths
(`tiger::paw` or `lion::paw`) so the arm can name one subtree.

A `match` is exhaustive over the enum's selectable members — its concrete leaves.
Every selectable leaf must be covered by exactly one arm, where a category arm
covers all leaves under it. A leaf left uncovered (reported by its full path), an
arm walking to no member, or two arms covering the same leaf (a repeated member, or
a leaf and an enclosing category) is a compile error. There is no wildcard arm;
because the enum is fixed in source, listing every leaf is always possible, and the
compiler holds a `match` to it so adding a member surfaces every `match` that must
handle it.

A set whose members are managed at runtime is not an enum — it is a saved
resource referenced by a field. An enum is fixed in source, which is what lets a
`match` over it be exhaustive.

`enum` takes `pub` like any declaration.
