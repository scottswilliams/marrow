# Data Modeling

## Custom identity allocation

Saved-root identities are allocated as a single auto-incrementing `int`. Custom
identity allocation policies wait until single-`int` allocation is fully
exercised in practice.

See [`../data-modeling.md`](../data-modeling.md) for identity keys as they work
today.
