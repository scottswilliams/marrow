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
