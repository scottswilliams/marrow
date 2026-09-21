//! `exists(value)` over a `T?` through the production path: it answers presence for
//! any optional expression and narrows nothing.

use crate::common::Project;

const MAYBE: &str = "fn maybe(present: bool): int? {\n\
    \x20   if present {\n\
    \x20       return 7\n\
    \x20   }\n\
    \x20   return absent\n\
    }\n";

#[test]
fn exists_answers_presence_for_an_optional_value() {
    let workspace = Project::single(&format!(
        "{MAYBE}\n\
         pub fn present(): bool {{\n\
         \x20   return exists(maybe(true))\n\
         }}\n\n\
         pub fn missing(): bool {{\n\
         \x20   const v = maybe(false)\n\
         \x20   return exists(v)\n\
         }}\n"
    ))
    .materialize("exists-optional");
    for (export, expected) in [("present", "true"), ("missing", "false")] {
        let output = workspace.marrow(&["run", export, "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{export}: {stdout}");
        assert!(
            stdout.contains(&format!(r#""data":{expected}"#)),
            "{export}: {stdout}"
        );
    }
}

/// The probe establishes no narrowing: the value keeps its optional type inside the
/// guarded block, so an arithmetic use there is a `check.type`.
#[test]
fn exists_establishes_no_narrowing() {
    let diagnostics = Project::single(&format!(
        "{MAYBE}\n\
         pub fn f(): int {{\n\
         \x20   const v = maybe(true)\n\
         \x20   if exists(v) {{\n\
         \x20       return v + 1\n\
         \x20   }}\n\
         \x20   return 0\n\
         }}\n"
    ))
    .try_image()
    .expect_err("the optional is not narrowed");
    let row = diagnostics.only("check.type");
    assert_eq!(row.line(), 11, "{:?}", diagnostics.all());
}

/// A value that is not optional has no presence to probe.
#[test]
fn exists_over_a_present_value_is_refused() {
    let diagnostics = Project::single("pub fn f(): bool {\n    return exists(3)\n}\n")
        .try_image()
        .expect_err("a present value is refused");
    let row = diagnostics.only("check.type");
    assert_eq!(
        (row.line(), row.column()),
        (2, 19),
        "{:?}",
        diagnostics.all()
    );
}
