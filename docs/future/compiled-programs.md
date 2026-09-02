# Compiled programs

Marrow compiles a project to a program image, verifies the image, and runs it
on a stack VM.

## Today

Compilation turns one closed source graph into an immutable image without
opening a store or the network. The image holds concrete executable facts:
types, monomorphized functions, exports, source maps, and the durable contract
the program uses. The same source and toolchain produce the same image and the
same identity.

The compiler emits image bytes but does not mint a verified image. The verifier
accepts an image before the VM runs it, and the VM runs only an accepted image,
so a compiler defect cannot by itself admit an unchecked program. A malformed,
noncanonical, or overlarge image fails before the VM starts, with an `image.*`
code ([error codes](../error-codes.md)). Verification has explicit byte,
list, and function bounds ([execution limits](../language/execution-limits.md)).
An image is read only by the toolchain that produced it; there is no stable
ABI. The VM is qualified on one target.

## Direction

A compact bytecode and a reference VM remain the design. Native code
generation, a JIT, an optimizer, a stable binary package ABI, and compiler
self-hosting are not planned.

A program's host effects precede its durable access. The host phase closes at
the first durable operation and stays closed
([path effects and authority](path-effects-and-authority.md)).

The compiler keeps few representations: lossless syntax facts, one resolved
source-near intermediate form, and the image draft. It adds a control-flow
graph, an SSA form, or a pass framework only when an implemented feature makes
source-near analysis insufficient.

Closures and higher-order forms are outside the image model
([general-purpose language](general-purpose-language.md)). No instruction
encoding receives a compatibility promise before the acceptance programs run
on it.

## Image encoding version

Today's image is version 0. Each list in the image (types, functions, strings,
and the rest) records its entry count, and the bytecode records each reference
into a list, as a 16-bit integer, so one list holds at most 65,535 entries.
The shipped bounds sit far below that; the widest is 8,192 entries
([execution limits](../language/execution-limits.md)).
Any bound can be raised toward 65,535 without a format change; an older
toolchain rejects a larger image instead of misreading it.

One image in which one list exceeds 65,535 entries is the version-1 decision. It
is deferred until a real program needs it. Version 1 bumps the container
version byte, mints a new digest kind selected by that byte, and widens the
counts and operands to 32 bits. A toolchain reads exactly its own image
version, because an image is regenerated from source across a toolchain
update. A version-1 digest validates no version-0 bytes, and the reverse.

## Evidence

Storeless and durable acceptance programs run only after decode and
verification. Mutation corpora, deep and wide compiler workloads, generic
allocation measurements, and clean rebuilds are available before the format
receives a compatibility promise.
