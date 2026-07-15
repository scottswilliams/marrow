# Tools

The beta line ships a thin command-line tool while the prototype command
families are refounded lane by lane. Tools consume parser and syntax facts;
they do not define a second language or saved-data model.

- [CLI Reference](cli.md) covers `init`, `fmt`, `run`, `test`, `--version`,
  `--help`, and the typed not-yet-supported response every refounding command
  name reports.
- [Tests](tests.md) covers `marrow test`: discovering and running `test`
  declarations and reading its report.
- [Projects](projects.md) covers the project layout, the `marrow.toml` manifest
  contract, and path-derived module identity.
- [TypeScript client](typescript-client.md) covers `marrow client typescript`:
  the generated strict client, the pinned Node supervision module, and the
  closed loss classification for calls whose reply is lost.

The prototype check/data/doctor/evolve/backup/restore tooling and the
surface/client/serve commands were deleted at B00; each returns through its
refounding lane. `run`, `test`, and `client typescript` have been refounded on
the new pipeline. See [Project status](../status.md).
