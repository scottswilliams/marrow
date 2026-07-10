# marrow CLI Contributor Notes

This binary hosts current commands for checking, running, testing, formatting,
data inspection, evolution, serving, backup, and restore. Each command parses
arguments into typed inputs, calls a lower-level operation, and renders the
result.

One format-aware renderer owns text, JSON, and JSONL output for each diagnostic
boundary. `term_style` is the single painting owner, and one named usage-exit
owner handles command-line usage failures. Machine-readable consumers never
scrape stderr prose. Prefer typed state over behavior-selecting booleans and
keep reusable engine logic below the binary.

The current command set, `ServeMode`, and surface routes are implementation
state. The binary owns no language, semantic path, public URI, or authorization
meaning. Embedded and served profiles must consume the same compiler-owned
semantics.

Map: [docs/implementation/cli.md](../../docs/implementation/cli.md).
