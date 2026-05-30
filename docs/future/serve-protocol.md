# Serve Protocol

## Planned read surfaces

These extend the read-only server; none would write managed data.

- Local IPC over Unix sockets or Windows named pipes. Loopback TCP is the v1
  transport.
- Evaluating one checked, non-mutating query in a session, and registering a
  client's own in-memory trees for read-only inspection.

See [`../serve-protocol.md`](../serve-protocol.md) for the operations the server
speaks today.
