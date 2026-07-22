# Compiled programs

This page is future direction. Marrow currently executes a checked in-memory
representation with a tree-walking interpreter.

## Goal

Compilation should turn one exact closed source graph into a reproducible
immutable ProgramImage without opening a user store or consulting the network.
The image should hold only concrete executable facts: types, functions,
direct-call bounds, host imports, exports, source maps, and any durable
contract the program uses. Source-level generic and effect schemes remain
compiler analysis; executable authority cannot rest on an unverified universal
compiler claim.

An independent bounded verifier should accept an image before a bytecode VM
executes it. The compiler emits image bytes but does not mint a verified image:
producing bytes and establishing that they are safe to run are separate
responsibilities. The VM should accept only an image an independent verifier has
accepted, so a compiler defect cannot by itself admit an unchecked program.

## Constraints

- Source resolution and type/effect checking complete before executable
  lowering.
- Every accepted function has one complete lowered body.
- The compiler retains a small number of persisted representations: lossless
  syntax facts, one typed source-near resolved intermediate representation, and
  the working image draft. It does not keep a separate checked-syntax clone, a
  control-flow graph, an SSA or mid-level form, an optimizer or pass framework,
  or a query engine unless an implemented feature makes source-near structured
  analysis insufficient.
- The same explicit source and toolchain-semantic inputs produce the same image
  and identity.
- The ProgramImage is exact-toolchain-private and has no stable ABI. The VM is
  qualified on one target rather than presented as a portable virtual machine.
  Bytecode encodings are private to the producing toolchain.
- Malformed, noncanonical, overlarge, or incompatible images fail before VM or
  host entry.
- Verification has explicit time, depth, graph, function, and byte limits.
- A program's host effects precede any durable access. The host phase is
  monotone: it closes at entry to a mutating transaction or at the first
  read-only durable operation and never reopens. This phase boundary is
  reconstructed by the verifier rather than trusted from the compiler.
- VM values, calls, allocation, faults, and evaluation order have deterministic
  language behavior even when their physical representation changes. Closures and
  higher-order forms are deferred and are not assumed by the beta image model.
- Presence, exact mutation, transaction ownership, and bounded traversal laws
  settle through the complete acceptance corpus before their instruction
  encodings or relevant retained-data structural encodings receive compatibility
  promises. Operation and control laws are not folded into a data-contract
  identity merely because the kernel enforces them.

A compact bytecode and reference VM are the chosen direction for the beta.
Native code generation, a JIT, an optimizer program, a stable binary package
ABI, and compiler self-hosting are not required.

## Table-count representation and the u32 ring

**Current.** Each image table encodes its actual entry count, and each
cross-table reference in the bytecode (a function, type, field, site, enum, or
local index), as a big-endian `u16`. The hard ceiling of this representation is
65,535 entries per table. The shipped decode bounds sit far below it (record
types, enum types, functions, and collection types at 4,096; the operation-site,
durable-member, and string tables at 8,192), and the whole-image byte ceiling
(512 KiB) binds first — at roughly 6,200 declared durable fields, or roughly
17,000 once eager per-field operation-site emission is retired. No compilable
program can therefore populate a table past `u16`. Raising any single bound
toward 65,535 is a monotone decode-guard widen with no format change: an image a
narrower bound accepted a wider one still accepts byte-for-byte, and an older
toolchain meeting a larger image refuses it with a typed bound rejection rather
than misreading it.

**Future.** Crossing `u16` — a program whose type, function, or enum population
alone exceeds 65,535 entries in one image — is the versioned format decision.
It is deferred until a lane first has such a reachable program, and re-checked
at the durable-floor freeze (Q02), because building a second encoding exercised
by no reachable program would be unused machinery. When it lands it is image
version 1: the container version byte becomes `0x01`, the digest domain kind
becomes `marrow.image.v1` (selected by the version byte, not a fixed constant),
and the table counts and cross-table bytecode operands widen to `u32` (or a
length-delimited varint). The evolution is reject-only: a toolchain reads
exactly its own image version, with no read-old shim, because the image is
exact-toolchain-private and is regenerated and rebound from source across a
toolchain update. A version-1 digest can never validate version-0 bytes (or the
reverse), because the digest kind is domain-separated per version. The `u16`
ceiling is a per-image, per-table bound; a program family spanning millions of
functions across many images and toolchains never reaches version 1 on that
count alone — only a single image whose one table exceeds 65,535 does.

## Evidence target

Storeless and durable acceptance programs must execute only after decode and
independent verification. Mutation corpora, deep and wide compiler workloads,
generic allocation measurements, and clean rebuilds must be available before the
format receives a compatibility promise. Grammar and image work should be driven
by the fixed whole-program experience corpus for Graph Report and Club Locker,
with each implemented feature extending its executable slice. Before
compatibility freezes, the complete valid and invalid programs, diagnostics,
formatting, maintenance edits, and terminal or generated-host boundary must pass;
an architecture-only vertical is not enough.

Related direction: [durable programming](durable-programming.md) for the durable
operations, transactions, and traversal an image encodes, and [project
status](../status.md) for what is current, legacy, and future.
