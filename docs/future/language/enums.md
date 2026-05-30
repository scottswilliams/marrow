# Enum hierarchies

Behavior designed but not yet implemented. Flat enums are live; see
[Enums](../../language/enums.md). This page covers the deferred hierarchy layer.

Members may nest into a tree; a value is then the selected member, `is` tests a
subtree (`pet is Cat::tiger`), and a member marked `abstract` is a category that
cannot be selected directly. A flat enum is a one-level tree.
