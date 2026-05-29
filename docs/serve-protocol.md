# Serve Protocol

`marrow serve` is a long-lived, read-only owner of a project's saved data. It
opens the store the project's `marrow.json` selects and answers newline-delimited
JSON requests over a loopback TCP connection. It never writes managed data: it is
an inspection surface, not Marrow's application API. Managed data changes come
only from checked Marrow execution (`marrow run` or an embedded runtime) and from
explicit repair and migration commands.

It is distinct from [`marrow lsp`](lsp.md), the editor language server, which
speaks `Content-Length`-framed JSON-RPC over stdio for a different purpose. For
the `marrow serve` command's flags and exit behavior, see
[cli.md](cli.md#marrow-serve).

## Starting the server

```
marrow serve [--port <port>] <projectdir>
```

The listener binds `127.0.0.1` only. `--port 0` (the default) lets the OS choose
a free port. The bound address is printed to stdout on startup, then the server
blocks on accept:

```
$ marrow serve --port 0 ./myproject
marrow serve listening on 127.0.0.1:52207
```

A client (or a test) reads that line to discover the chosen port. A project with
no saved data yet serves an empty store; inspection never creates the backing
file.

Usage failures exit `2` before the server starts: a missing project directory, a
non-numeric `--port`, an unknown option, or more than one project directory. A
bind failure or an unreadable listen address exits `1`.

## Framing

Each request is one JSON object on its own line, terminated by `\n`. Each reply
is one JSON object on its own line. The connection is a request/reply loop: send
a line, read a line, repeat, until the client hangs up (a clean EOF ends the
connection, not the server).

- Blank lines are ignored (no reply).
- Requests may be pipelined: replies come back in order, one per request line.
- The server accepts connections one at a time and serves each to completion.
- A connection has a 30-second read timeout. A client that connects and then
  stalls has its connection closed (like a hang-up); the accept loop moves on.
- A request line may be up to 64 MiB; a longer line without a newline earns a
  `protocol.malformed` reply (see below) and the connection stays open.

## Reply envelope

Every reply echoes the request's `id` (whatever JSON value was sent, or `null`
if the request had none or could not be parsed). On success:

```json
{"id": 1, "ok": { /* operation result */ }}
```

On failure:

```json
{"id": 1, "error": {"code": "protocol.bad_request", "message": "..."}}
```

`handle_request` never fails: every error, protocol or storage, becomes an
`error` reply. Clients branch on the presence of `ok` versus `error`, never on
the message text.

## Operations

A request is `{"id": <any>, "op": "<name>", ...}`. The four operations are the
saved-tree reads. All but `saved_roots` take a `path` (see
[Path encoding](#path-encoding)).

### `saved_roots`

The project's saved root names, in store order.

```
REQ   {"id": 1, "op": "saved_roots"}
REPLY {"id":1,"ok":{"roots":["books"]}}
```

An empty store replies `{"id":1,"ok":{"roots":[]}}`.

### `saved_children`

The distinct immediate children directly below `path`, in Marrow order. Each
child is one of:

- `{"key": <key>}` — a record key or index key (see [Key encoding](#key-encoding));
- `{"name": "<member>"}` — a field, child-layer, or index name.

The store cannot tell a field, layer, and index name apart from bytes alone (the
schema does that), so all three come back as `{"name": ...}`.

```
REQ   {"id": 2, "op": "saved_children", "path": [{"root": "books"}]}
REPLY {"id":2,"ok":{"children":[{"key":{"int":1}},{"key":{"int":2}}]}}

REQ   {"id": 3, "op": "saved_children", "path": [{"root": "books"}, {"key": {"int": 1}}]}
REPLY {"id":3,"ok":{"children":[{"name":"tags"},{"name":"title"}]}}
```

Record and index keys sort before named members at one tree level; that order is
preserved in the reply.

### `saved_get`

The presence at an exact saved path plus its stored value.

```
REQ   {"id": 4, "op": "saved_get",
       "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "title"}]}
REPLY {"id":4,"ok":{"presence":"value_only","value":"TW9ydA=="}}
```

`presence` is one of four states:

| `presence`           | Meaning                                  |
|----------------------|------------------------------------------|
| `absent`             | nothing stored at or below this path     |
| `value_only`         | a value, no children                     |
| `children_only`      | children, no value of its own            |
| `value_and_children` | both                                     |

`value` is the stored bytes as standard padded base64, or `null` when no value
is stored at that exact path. The bytes are the store's raw canonical encoding;
the client decodes them with the field's schema type — the protocol does not
interpret them. `"TW9ydA=="` above is the string `Mort`.

A record node has children but no value of its own:

```
REQ   {"id": 5, "op": "saved_get", "path": [{"root": "books"}, {"key": {"int": 1}}]}
REPLY {"id":5,"ok":{"presence":"children_only","value":null}}
```

An absent path:

```
REQ   {"id": 6, "op": "saved_get",
       "path": [{"root": "books"}, {"key": {"int": 99}}, {"field": "title"}]}
REPLY {"id":6,"ok":{"presence":"absent","value":null}}
```

### `saved_walk`

Up to `limit` `(path, value)` entries in the subtree at `path`, in Marrow order,
plus whether the page was truncated.

```
REQ   {"id": 7, "op": "saved_walk", "path": [{"root": "books"}], "limit": 1}
REPLY {"id":7,"ok":{"entries":[{"path":"AWJvb2tzAAICgAAAAAAAAAEDdGFncwAEAoAAAAAAAAAB","value":"ZmF2b3JpdGU="}],"truncated":true}}

REQ   {"id": 8, "op": "saved_walk", "path": [{"root": "books"}], "limit": 100}
REPLY {"id":8,"ok":{"entries":[
         {"path":"AWJvb2tzAAICgAAAAAAAAAEDdGFncwAEAoAAAAAAAAAB","value":"ZmF2b3JpdGU="},
         {"path":"AWJvb2tzAAICgAAAAAAAAAEDdGl0bGUA","value":"TW9ydA=="},
         {"path":"AWJvb2tzAAICgAAAAAAAAAIDdGl0bGUA","value":"U291cmNlcnk="}],
       "truncated":false}}
```

In a `saved_walk` entry, both `path` and `value` are base64. Unlike a request
`path` (an array of segment objects) and unlike `saved_get`'s `value` (decodable
with the schema type), the walk `path` bytes are the store's opaque internal key:
in v1 the client does not decode them. They serve to order, compare, and resume.

Paging:

- `limit` is required and must be a JSON integer; omitting it is a
  `protocol.bad_request`.
- `limit` is clamped to a server maximum of 10000; a larger request is silently
  capped, not rejected, so an unbounded request cannot force a huge scan.
- `truncated` is `true` when more entries remained past the limit. There is no
  cursor token in v1: a client pages by walking deeper subtrees (issue
  `saved_walk` against narrower paths) rather than by an offset.

## Path encoding

A request `path` is a JSON array of segment objects, ordered root-first. Each
segment is a one-field object tagged by its kind:

| Segment            | Meaning                                  | Example (`.mw`)        |
|--------------------|------------------------------------------|------------------------|
| `{"root": "<s>"}`  | the saved root (always first)            | `^books`               |
| `{"key": <key>}`   | a record (identity) key                  | the `1` in `^books(1)` |
| `{"field": "<s>"}` | a declared field name                    | `^books(1).title`      |
| `{"layer": "<s>"}` | a declared child-layer name              | `versions`             |
| `{"index": "<s>"}` | a declared index name                    | `byShelf`              |
| `{"index_key": <key>}` | a key value inside an index or layer | a key under `byShelf`  |

`root`, `field`, `layer`, and `index` carry a string. `key` and `index_key`
carry a [key object](#key-encoding). An empty array names the level above the
roots.

Example: `^books(1).title` is

```json
[{"root": "books"}, {"key": {"int": 1}}, {"field": "title"}]
```

## Key encoding

A key (in a `key` or `index_key` segment, and in `saved_children` output) is a
one-field object tagged by the key's type:

| Tag          | JSON form                  | Notes                                       |
|--------------|----------------------------|---------------------------------------------|
| `int`        | integer                    | `{"int": 1}`                                |
| `bool`       | boolean                    | `{"bool": true}`                            |
| `str`        | string                     | `{"str": "fiction"}`                        |
| `date`       | integer                    | days since 1970-01-01                       |
| `duration`   | string                     | nanoseconds in a decimal string             |
| `instant`    | string                     | nanoseconds since the epoch, decimal string |
| `bytes`      | base64 string              | `{"bytes": "AAEC/w=="}`                      |

`duration` and `instant` are carried as decimal strings because they are 128-bit
and JSON numbers cannot hold them. Sending one as a number is a
`protocol.bad_request`. `bytes` uses the same strict base64 as values.

## Base64

Values, `bytes` keys, and `saved_walk` paths use standard RFC 4648 base64 (the
`+`/`/` alphabet) with required `=` padding. Decoding is strict: unpadded or
over-padded text is rejected. There is exactly one base64 dialect across the
serve surface and the runtime — `Zm8` and `Zg====` are invalid; the padded
`Zm8=` is valid.

## Error replies

Errors carry a stable dotted `code`; the `message` is human text and is not part
of the contract. The protocol-level codes:

| Code                   | When                                                                 |
|------------------------|----------------------------------------------------------------------|
| `protocol.malformed`   | the line is not JSON, the request is not an object, or it has no string `op` |
| `protocol.unknown_op`  | a known envelope but an `op` the server does not implement           |
| `protocol.bad_request` | a known `op` with bad arguments — missing or non-array `path`, an unknown segment kind, a segment that is not a one-field object, an unknown key type, a wide-integer key that is not an integer string, invalid base64, or a `saved_walk` without an integer `limit` |

A request that parses but cannot be answered by the store carries the store's own
`store.*` code through unchanged (for example `store.corrupt_path` on an
undecodable stored key). See [Errors](error-codes.md) for the `store.*` family.

Observed replies:

```
{"id": null, "op": <unparseable>}
  -> {"error":{"code":"protocol.malformed","message":"key must be a string at line 1 column 2"},"id":null}

{"id": 10, "what": true}
  -> {"error":{"code":"protocol.malformed","message":"request is missing a string `op`"},"id":10}

{"id": 11, "op": "frobnicate"}
  -> {"error":{"code":"protocol.unknown_op","message":"unknown operation `frobnicate`"},"id":11}

{"id": 12, "op": "saved_get", "path": [{"frob": "x"}]}
  -> {"error":{"code":"protocol.bad_request","message":"unknown path segment `frob`"},"id":12}

{"id": 13, "op": "saved_get", "path": [{"root": "books"}, {"key": {"frob": 1}}]}
  -> {"error":{"code":"protocol.bad_request","message":"unknown key type `frob`"},"id":13}

{"id": 14, "op": "saved_get", "path": [{"root": "books"}, {"key": {"bytes": "!!!"}}]}
  -> {"error":{"code":"protocol.bad_request","message":"`bytes` is not valid base64"},"id":14}

{"id": 15, "op": "saved_get"}
  -> {"error":{"code":"protocol.bad_request","message":"request is missing `path`"},"id":15}

{"id": 16, "op": "saved_walk", "path": [{"root": "books"}]}
  -> {"error":{"code":"protocol.bad_request","message":"`saved_walk` requires an integer `limit`"},"id":16}
```

A line that cannot be parsed gets a `protocol.malformed` reply with `id: null`
(the id is unknown), and the connection stays open for the next request. The same
holds for a non-UTF-8 line or one over the 64 MiB size limit.

## Security

The listener binds loopback (`127.0.0.1`) only. The protocol has no
authentication or transport security, so binding beyond loopback is not supported
by the server; it would require both. The read-only guarantee is what lets serve
be a long-lived shared owner of the store: several local tools can read one
live-owned store without risking a write through this surface.

## Status

What works today: the four read operations (`saved_roots`, `saved_children`,
`saved_get`, `saved_walk`) over loopback TCP, with the path/key/base64 encodings
and `protocol.*` / `store.*` error replies described above.

Designed read extensions that are not yet implemented — a resumable cursor for
`saved_walk`, local IPC over Unix sockets or Windows named pipes, and two
read-only session extensions — are described in
[future/serve-protocol.md](future/serve-protocol.md). None would write managed
data.
