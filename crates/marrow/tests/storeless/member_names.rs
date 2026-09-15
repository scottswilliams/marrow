//! `marrow check` and `marrow run` agree on a repeated parameter name: the
//! declaration is refused with a located `check.name_conflict` before any image is
//! built, so `f(1, 2)` never executes with its second slot standing in for `a`.

use crate::common::Project;

const REPEATED_PARAMETER: &str = "module main\n\n\
fn f(a: int, a: int): int {\n    return a\n}\n\n\
pub fn main(): int {\n    return f(1, 2)\n}\n";

#[test]
fn check_and_run_refuse_a_repeated_parameter_at_its_name() {
    let workspace = Project::single(REPEATED_PARAMETER).materialize("repeated-parameter");

    let check = workspace.marrow(&["check"]);
    let report = String::from_utf8_lossy(&check.stderr);
    assert!(
        !check.status.success(),
        "`marrow check` accepted a repeated parameter: {report}"
    );
    assert!(
        report.contains("src/main.mw:3:14: check.name_conflict"),
        "`marrow check` locates the conflict at the second `a`: {report}"
    );

    let run = workspace.marrow(&["run", "main", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        !run.status.success(),
        "`marrow run` executed a repeated parameter: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.name_conflict""#)
            && stdout.contains(r#""span":{"column":14,"line":3}"#),
        "`marrow run` refuses at the same span, before any image is verified: {stdout}"
    );
    assert!(
        !stdout.contains("image.table"),
        "the repeat never reaches the verifier: {stdout}"
    );
}
