# marrow-project — Agent Notes

Project discovery and config: `marrow.json`, source roots, path-to-module resolution, and the project
digest. Deliberately small so the CLI, editors, and language services agree on one loader.

A closed `ConfigErrorKind` taxonomy carries typed `ConfigPathField`/`ConfigPathViolation` payloads;
fail-closed path checks (absolute, `..`, glob, NUL) make traversal-escape unrepresentable; every raw
struct sets `deny_unknown_fields`; src holds zero `unwrap`/`panic`. Every exported type derives
`Debug`. Precedent: C-VALIDATE, BurntSushi typed config errors.

Map: [docs/implementation/cli.md](../../docs/implementation/cli.md).
