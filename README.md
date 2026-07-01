# Marrow

Marrow is a small typed language with built-in saved data.

```mw
module app::tasks

resource Task
    required title: string
    status: string
store ^tasks(id: int): Task

pub fn complete(id: Id(^tasks)): bool
    if not exists(^tasks(id))
        return false

    ^tasks(id).status = "done"
    return true
```

Marrow has one data model: a resource is a typed tree. The same resource shape
can be local or saved, and `^` marks saved data.

## Start Here

- [Install](docs/install.md) covers the source-install path and supported
  platforms.
- [Quickstart](docs/quickstart.md) creates a project, runs it, and inspects
  saved data.
- [Stability Contract](docs/stability.md) names the v0.1 release surfaces and
  non-stable surfaces.
- [Reference Index](docs/README.md) links the language, tooling, and
  architecture docs.
- [Changelog](CHANGELOG.md) tracks v0.1.0 release notes and later unreleased changes.

## References

- [Language](docs/language/) defines `.mw` syntax, types, resources, saved
  data, control flow, builtins, standard library contracts, and grammar.
- [Implementation Map](docs/implementation/) is the code-truth architecture map:
  what each crate and module does and where to read it, plus the
  [backend contract](docs/backend-contract.md) the store satisfies.

## Shape

The first implementation target is deliberately small:

- native `.mw` parser, formatter, checker, and runtime model;
- resources as typed local and saved trees;
- native local storage behind a simple ordered-tree backend contract;
- CLI and language services built from checked program facts;
- no alternate language modes in the default product;
- no bundled external database adapters.

## Scope And Security

Marrow treats project source, `marrow.json`, `marrow.lock`, native store
files, backup archives, CLI arguments, and host-provided environment, filesystem,
clock, log, and output channels as untrusted inputs. The compiler and runtime
fail closed with typed diagnostics or `run.*` / `store.*` faults; they do not
claim process sandboxing for code that an embedding chooses to run.

Store and backup checksums detect accidental corruption, truncation, or a file
from the wrong project state. They are not authentication, tamper proofing,
encryption, or an authorization boundary.

Host capabilities are the determinism and embedding boundary:

| Capability | Runtime boundary |
|---|---|
| none | deterministic helper or assertion; no host access |
| `Clock` | caller-supplied run timestamp |
| `Environment` | caller-supplied environment map |
| `Log` | caller-supplied log sink |
| `Filesystem` | real filesystem access through `std::io` |
| `Maintenance` | explicit repair/admin operations only |

## License

Apache-2.0
