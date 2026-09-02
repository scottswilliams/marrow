# Future direction

This directory records direction that follows from the [vision](../vision.md)
and is unimplemented today. Nothing here is current behavior. The
[language reference](../language/README.md) defines current behavior, and
[status](../status.md) separates current work from future work.

Language and packages:

- [General-purpose language](general-purpose-language.md): what a storeless program still lacks.
- [Packages](packages.md): exact path and Git dependency edges over an offline cache.
- [Source standard library](source-standard-library.md): library code written in Marrow.

Compilation and admission:

- [Compiled programs](compiled-programs.md): the image pipeline and its open format decision.
- [Admission and activation](admission-and-activation.md): how a changed program meets an existing store.

Durable model and paths:

- [Durable programming](durable-programming.md): what the durable model still adds, and its open forks.
- [Semantic paths](semantic-paths.md): the distinct identities a durable declaration has.
- [Path effects and authority](path-effects-and-authority.md): authority attached to paths and effects.
- [Data coexistence](data-coexistence.md): durable data beside external systems.

Applications and serving:

- [Local applications](local-applications.md): the runner, client, and bundle for one machine.
- [Served execution](served-execution.md): several terminals, public paths, and concurrent writers.

A future page states what is current today with a link, the direction, and the
evidence that would make the direction current. It contains no proposed `.mw`
syntax, manifest field, instruction format, protocol schema, or diagnostic
catalog. When working code makes a behavior current, its rule moves into the
reference and the future page is deleted or reduced to what remains
unimplemented. Git history preserves abandoned proposals.
