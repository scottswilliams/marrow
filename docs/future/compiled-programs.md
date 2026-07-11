# Compiled programs

This page is future direction. Marrow currently executes a checked in-memory
representation with a tree-walking interpreter.

## Goal

Compilation should turn one exact locked source graph into a reproducible
immutable ProgramImage without opening a user store or consulting the network.
An independent bounded verifier should accept canonical image bytes before a
portable VM can execute them.

The image should contain only concrete executable facts: types, functions,
closure layouts, direct and indirect-call bounds, host imports, exports, source
maps, and any durable contract used by the program. Source-level generic and
effect schemes remain compiler analysis; executable authority cannot rest on an
unverified universal compiler claim.

## Constraints

- Source resolution and type/effect checking complete before executable
  lowering.
- Every accepted function has one complete lowered body.
- The same explicit source and toolchain-semantic inputs produce the same
  canonical image bytes and identity.
- Malformed, noncanonical, overlarge, or incompatible images fail before VM or
  host entry.
- Verification has explicit time, depth, graph, function, and byte limits.
- VM values, calls, closures, allocation, faults, and evaluation order have
  deterministic language behavior even if their physical representation changes.
- Presence, exact mutation, transaction ownership, bounded traversal, continuation,
  and effect laws settle through the complete acceptance corpus before their
  instruction encodings or relevant retained-data structural encodings receive
  compatibility promises. Operation and control laws are not folded into a
  data-contract identity merely because the kernel enforces them.

A compact bytecode and reference VM are the chosen direction for the beta.
Native code generation, a JIT, optimizer program, stable binary package ABI,
and compiler self-hosting are not required.

## Evidence target

Storeless and durable acceptance programs must execute only after canonical
decode and independent verification. Mutation corpora, deep/wide compiler
workloads, closure/generic allocation measurements, and clean rebuilds must be
available before the format receives a compatibility promise. Grammar and image
work should be driven by the fixed whole-program experience corpus for Graph
Report and Club Locker, with each implemented feature extending its executable
slice. Before compatibility freezes, the complete valid and invalid programs,
diagnostics, formatting, maintenance edits, and terminal/generated-host boundary
must pass; an architecture-only vertical is not enough.
