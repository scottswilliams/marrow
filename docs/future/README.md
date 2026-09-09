# Future direction

This directory records direction that follows from the [vision](../vision.md).
Each page opens with what the toolchain does today, then the direction that is
not implemented and the evidence that would make it current. The
[language reference](../language/README.md) defines current behavior, and
[status](../status.md) separates current work from future work.

[Beta scope](../vision.md#beta-scope) selects the immediate product boundary.
Presence in this directory does not make a capability a beta prerequisite.
Local source reuse, bounded terminal text I/O, ordinary enum composition, coherent tests,
additive local updates and complete backup/restore belong to that boundary.
Remote packages, a source-library portfolio, implicit presence proofs and served
execution remain deferred.

Language and packages:

- [General-purpose language](general-purpose-language.md): what a storeless program still lacks.
- [Packages](packages.md): local source reuse and deferred remote acquisition.
- [Source standard library](source-standard-library.md): library code written in Marrow.

Compilation and admission:

- [Compiled programs](compiled-programs.md): bounded verified execution and format compatibility.
- [Admission and activation](admission-and-activation.md): how a changed program meets an existing store.

Durable model and paths:

- [Durable programming](durable-programming.md): selected composition, invocation and data-lifetime rules.
- [Semantic paths](semantic-paths.md): the distinct identities a durable declaration has.
- [Path effects and authority](path-effects-and-authority.md): authority attached to paths and effects.
- [Data coexistence](data-coexistence.md): durable data beside external systems.

Applications and serving:

- [Local applications](local-applications.md): the runner, client, and bundle for one machine.
- [Served execution](served-execution.md): several terminals and public paths over one store with serial mutating invocations.

A future page contains no proposed `.mw` syntax, manifest field, instruction
format, protocol schema, or diagnostic catalog. When working code makes a
behavior current, its rule moves into the reference and the future page is
deleted or reduced to what remains unimplemented. Git history preserves
abandoned proposals.
