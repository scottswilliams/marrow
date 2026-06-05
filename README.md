# Marrow

Marrow is a small typed language with built-in saved data.

```mw
module app::tasks

resource Task at ^tasks(id: int)
    required title: string
    status: string

pub fn complete(id: Id(^tasks)): bool
    if not exists(^tasks(id))
        return false

    ^tasks(id).status = "done"
    return true
```

Marrow has one data model: a resource is a typed tree. The same resource shape
can be local or saved, and `^` marks saved data.

## References

- [Language](docs/language/) defines `.mw` syntax, types, resources, saved
  data, control flow, builtins, standard library contracts, and grammar.
- [Implementation And Backends](docs/implementation.md) defines the
  language/database kernel, project configuration, saved paths, managed writes,
  native storage, tooling, and capability profiles.

## Shape

The first implementation target is deliberately small:

- native `.mw` parser, formatter, checker, and runtime model;
- resources as typed local and saved trees;
- native local storage behind a simple ordered-tree backend contract;
- CLI and language services built from checked program facts;
- no alternate language modes in the default product;
- no bundled external database adapters.

## License

Apache-2.0
