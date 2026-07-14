# Operations

There are no supported operational procedures on the beta line yet: the
prototype's native-store lifecycle, recovery, backup, and restore commands were
deleted at B00, and their replacements arrive with the durable lifecycle lanes
(store provision/open, admission and activation, audit, logical backup, and
fresh-store restore). Operations pages return here as that behavior lands.

Marrow does not install a daemon, service manager, replication layer, or
high-availability control plane. See [Project status](../status.md) for the
current state.
