# Operations

The implemented operational surface is the local store lifecycle. `marrow
import` provisions a native store and populates it from a JSONL corpus, and
`marrow run <export> --store <dir>` runs a durable export against a provisioned
store; both drive a release-verified companion runner under the store's
single-owner lock. Provisioning is the only path that creates a store; ordinary
and recovery opens are existing-only and refuse missing or invalid engine
files. The local commit-recovery slice is implemented: an indeterminate engine
commit is classified after a fresh open and audit under the continuously held
owner lock. Schema evolution, logical backup, and fresh-store restore remain
future work, and the `backup` and `restore` command names report
`cli.command_unsupported`. Operations pages return here as that behavior
lands.

Marrow does not install a daemon, service manager, replication layer, or
high-availability control plane. See [Project status](../status.md) for the
current state.
