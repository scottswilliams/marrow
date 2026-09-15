use crate::{test_ledger as ledger, test_project as project_capture};
use marrow_syntax::SourceSpan;

/// A write guarded by an older presence proof, with erasers before and inside the
/// loops that surround it, is refused once at the write itself — not once per
/// enclosing loop.
#[test]
fn overlapping_erased_intervals_report_the_guarded_write_once() {
    let source = r#"module main
resource R { required value: int }
store ^r[id: int]: R
fn erase(id: int) { delete ^r[id] }
fn condition(flag: bool): bool { return flag }
fn noop(value: int): int { return value }
pub fn write(id: int, flag: bool) {
    transaction {
        place p = ^r[id]
        if exists(p) {
            erase(id)
            while condition(flag) {
                erase(id)
                for i in 0..2 {
                    for j in 0..2 {
                        p.value = noop(id)
                    }
                }
                erase(id)
            }
        }
    }
}
"#;
    for statement in [
        "p.value = noop(id)",
        "const evaluated = noop(id)\n                        const copied: int? = p.value",
    ] {
        let source = source.replace("p.value = noop(id)", statement);
        let ids = ledger::ledger(&[
            "application .",
            "product R",
            "field R.value",
            "root r",
            "key r.id",
        ]);
        let input = project_capture::project_with_ids(&[("src/main.mw", &source)], Some(&ids));
        let result = crate::compile(&input);
        let Err(crate::CompileFailure::Diagnostics(rows)) = result else {
            panic!("the overlapping erased intervals must reject the write: {result:?}");
        };
        let start_byte = source.find("p.value").expect("the protected write exists");
        let write_span = SourceSpan {
            start_byte,
            end_byte: start_byte + "p.value".len(),
            line: source[..start_byte]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count() as u32
                + 1,
            column: (start_byte - source[..start_byte].rfind('\n').expect("a preceding line"))
                as u32,
        };
        assert_eq!(
            rows.as_slice().len(),
            1,
            "overlapping intervals report their use once",
        );
        let row = &rows.as_slice()[0];
        assert_eq!(row.code(), "check.requires_presence");
        assert_eq!(row.file().as_str(), "src/main.mw");
        assert_eq!(row.span(), write_span);
    }
}
