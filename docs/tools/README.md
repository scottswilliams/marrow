# Tools

The beta line ships a thin command-line tool while the prototype command
families are refounded lane by lane. Tools consume parser and syntax facts;
they do not define a second language or saved-data model.

- [CLI Reference](cli.md) covers `init`, `fmt`, `--version`, `--help`, and the
  typed not-yet-supported response every refounding command name reports.
- [Projects](projects.md) covers the project layout, the `marrow.toml` manifest
  contract, and path-derived module identity.

The prototype check/run/test/data/doctor/evolve/backup/restore tooling and the
surface/client/serve commands were deleted at B00; each returns through its
refounding lane. See [Project status](../status.md).
