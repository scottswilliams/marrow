# marrow CLI Contributor Notes

This crate adapts the compiler and runtime to public commands. The
[CLI reference](../../docs/tools/cli.md) owns command behavior; the
[implementation map](../../docs/implementation/README.md#tracing-a-command) owns
the current pipeline.

Capture projects through `marrow-project-fs::capture_project` with an empty
overlay. Render capture failures through its presentation facade; do not rebuild
discovery, identity or capture classification. Identity minting and interrupted
publication use the adapter's publication owner, settled before capture or
drawing entropy; the CLI does not implement another publication protocol.

`marrow check` drives the compiler once, including tests, and uses that drive's
complete diagnostics and test-inclusive image. Encode and independently verify
the image once to describe each export's reconstructed demand through the
compiler-owned `DurableNaming` join. Checking opens no store, executes no program
and grants no authority.

Durable command execution and read-only audit use the companion runner and
lifecycle admission. Currently a direct durable test gets a fresh attachment,
while a test driver calls exports at their own invocation boundaries. This split
is current behavior pending the [future test model](../../docs/future/durable-programming.md),
not a permanent design constraint. Test changes still require production-path
evidence. The client generator consumes the verified image's wire
interface and emits the supervision module verbatim under byte-exact drift tests.

The CLI has no direct or transitive dependency on `marrow-lsp` or `lsp-types`.
`tests/lsp_stdio.rs` enforces this dependency direction under all features; the
server's stdio behavior is tested in its own crate.

`outcome` is the typed CLI outcome owner: the four failure families (source
diagnostic, artifact rejection, source-mapped runtime fault, operational
error) stay distinct variants and never collapse into one channel.
`term_style` is the single painting owner, and one named usage-exit owner
handles command-line usage failures. Prefer typed state over
behavior-selecting booleans and keep reusable logic below the binary.

The binary owns no language, semantic path, public URI, or authorization
meaning. A command consumes compiler-owned semantics; it does not
reconstruct them here.
