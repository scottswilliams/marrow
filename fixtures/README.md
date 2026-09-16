# Fixture corpora

Two fixture roots exist, and the split is load-bearing.

`fixtures/v01/` (this directory) is the cross-crate corpus: complete Marrow
projects (`marrow.toml`, `src/`, optional `.marrow/ids`) plus standalone `.mw`
sources. `marrow-syntax`'s fuzz suite sweeps *every* `.mw` file under this root
and requires it to hold the total parser invariants, so only sources that are
meant to parse belong here. `crates/marrow/tests/` drives the projects under
`conformance/` through the built CLI.

`crates/marrow/tests/fixtures/v01/` is the `marrow` crate's private corpus,
loaded by `Project::from_fixture`. It deliberately contains unparseable and
unformatted sources used as negative cases, which is why it stays out of the
shared parse corpus above.

A fixture in either root needs a test that names it; an unnamed fixture is
deleted, not archived.

`conformance/graph_report` and `conformance/graph_report_lib` are one two-tree
pair: the library is a standalone project that checks, formats, and tests on its
own, and the application reaches its helpers through a `[dependencies]` alias by
relative path. A fixture pair relocates together, so the consuming path stays
relative and either tree may be driven from its own directory.

`.marrow/ids` is committed wherever a fixture declares durable state. The
compiler never mints identities, and `marrow run`'s entropy mint would rewrite
the file and make the fixture nondeterministic, so the ledger is frozen source:
fixed hex, `high-water 0`, one row per durable anchor.
