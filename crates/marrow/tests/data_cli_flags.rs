//! `marrow data` flag grammar: the shared `--format` parser the `data` subcommands
//! reuse. Asserted by its usage exit code and the rendered usage message.

mod support;
mod support_data;

use support_data::marrow;

#[test]
fn data_rejects_a_duplicate_format_flag() {
    // The `data` parsers share the one `--format` grammar, which rejects a repeated
    // flag uniformly rather than silently taking the last one.
    let output = marrow(&[
        "data", "roots", "--format", "json", "--format", "text", "missing",
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}
