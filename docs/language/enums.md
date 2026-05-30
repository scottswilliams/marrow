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
no type, no key parameters, and no nested body.

A member is written `Status::archived`. It is a value of the enum type, so it can
be the type of a field (`state: Status`), a parameter, a `var`, or a `const`, and
it can be compared with `==`. Equality is nominal: an enum value equals only a
value of the same enum. Comparing an enum to a raw string, or to a member of a
different enum, is a type error, as is any operator other than `==`/`!=`.

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

The header is `match <value>`, where the value is enum-typed. Each arm names one
member by its bare name — the scrutinee supplies the enum, so an arm is `archived`,
not `Status::archived` — followed by the indented block to run when the value is
that member.

A `match` over an enum is exhaustive: every member must have an arm. A missing
member, an arm naming a member the enum does not have, or two arms for the same
member is a compile error. There is no wildcard arm; because the enum is fixed in
source, listing every member is always possible, and the compiler holds a `match`
to it so adding a member surfaces every `match` that must handle it.

A set whose members are managed at runtime is not an enum — it is a saved
resource referenced by a field. An enum is fixed in source, which is what lets a
`match` over it be exhaustive.

`enum` takes `pub` like any declaration.
