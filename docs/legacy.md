# Legacy Architecture

This page inventories implemented mechanisms that remain reachable but are not
part of Marrow's intended architecture. It is not a usage guide or a stability
contract. [Project Status](status.md) owns their classification.

The current repository still contains:

- `surface` declarations that repeat selected durable fields and operations;
- checker-generated read, create, update, delete, collection, action, and
  computed-read operation families;
- opaque operation tags, JSON operation envelopes, and HTTP routes derived from
  those families;
- `marrow init --client`, the `marrow.json` `client` field, and automatic
  generated-client refresh;
- `marrow client typescript` and its current cursor profiles;
- `marrow serve`, including loopback and experimental remote profiles;
- linked-Rust surface read/write sessions built around the same model;
- the user-facing storage-cost and hidden-scan terminology attached to that
  stack.

These paths are current implementation facts only. New documentation and
features must not expand them or treat them as the basis of public paths,
authorization, generated application APIs, or a stable transport. The remote
profile's Bearer token authenticates one shared token; it is not
compiler-integrated path authorization.

Some legacy commands inspect or open the configured store. In particular,
client generation attempts a lenient read-only open to bind accepted catalog
facts when a live store is available. Read-only serving can use an absent store
as an empty committed view, while write serving can seed an absent store from a
committed lock. A failed serve watch recheck releases the prior session and
leaves the process unavailable until a later successful recheck. These details
explain current behavior; they do not establish a supported application model.

Removal of this inventory must be coordinated with removal or replacement of
the implementation so that the documentation remains truthful while the paths
are reachable.
