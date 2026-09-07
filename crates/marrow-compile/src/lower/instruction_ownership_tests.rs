use super::{BodyKind, Instr, LowerMode, PresenceObligation};
use crate::{test_ledger as ledger, test_project as project_capture};
use marrow_syntax::SourceSpan;
use std::cell::RefCell;
use std::ops::Range;

#[derive(Debug, Default)]
struct Observed {
    // Ordinary, concrete generic, proof-only, and test bodies, in that order.
    functions: [usize; 4],
    instructions: [usize; 4],
    copied: [usize; 4],
    calls: [usize; 4],
    intervals: Vec<(SourceSpan, Range<usize>)>,
}

thread_local! {
    static OBSERVED: RefCell<Option<Observed>> = const { RefCell::new(None) };
}

pub(super) fn observe(
    mode: LowerMode,
    kind: BodyKind,
    generic: bool,
    allocation: *const Instr,
    stored: &[Instr],
    calls: &[u16],
    obligations: &[PresenceObligation],
) {
    OBSERVED.with_borrow_mut(|observed| {
        let Some(observed) = observed else { return };
        let family = match (mode, kind, generic) {
            (LowerMode::Template, _, _) => 2,
            (_, BodyKind::Test, _) => 3,
            (_, _, true) => 1,
            (_, _, false) => 0,
        };
        assert!(!stored.is_empty(), "a completed body emits instructions");
        observed.functions[family] += 1;
        observed.instructions[family] += stored.len();
        observed.copied[family] += usize::from(allocation != stored.as_ptr());
        assert!(
            stored
                .iter()
                .filter_map(|instruction| match instruction {
                    Instr::Call(callee) => Some(*callee),
                    _ => None,
                })
                .eq(calls.iter().copied()),
            "the ordered call log contains each emitted Call exactly once",
        );
        observed.calls[family] += calls.len();
        for obligation in obligations {
            assert!(
                obligation.calls.start < obligation.calls.end
                    && obligation.calls.end <= calls.len(),
                "a retained obligation is nonempty and stays inside its function's call log",
            );
            observed
                .intervals
                .push((obligation.span, obligation.calls.clone()));
        }
    });
}

#[test]
fn completed_bodies_transfer_their_instruction_allocation() {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let source = r#"fn next(value: int): int { return value + 1 }
fn identity<T>(value: T): T { return value }
pub fn driver(): int {
    const value = identity(7)
    const flag = identity(true)
    if flag { return next(value) }
    return 0
}
test "a test body" { assert next(identity(2)) == 3 }
"#;
    let project = marrow_project::capture(
        &manifest,
        vec![marrow_project::CapturedFile::new(
            "src/main.mw".to_string(),
            source.as_bytes().to_vec(),
        )],
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture source");
    let mut copies = Vec::new();
    for include_tests in [false, true] {
        OBSERVED.set(Some(Observed::default()));
        let bytes = if include_tests {
            crate::compile_with_tests(&project)
                .expect("compile test bodies")
                .image
                .bytes
        } else {
            crate::compile(&project)
                .expect("compile functions")
                .image
                .bytes
        };
        assert!(!bytes.is_empty());
        let observed = OBSERVED.take().expect("observation enabled");
        assert_eq!(observed.functions, [2, 2, 1, usize::from(include_tests)]);
        assert!(observed.instructions[..3].iter().all(|count| *count > 0));
        assert_eq!(observed.instructions[3] > 0, include_tests);
        copies.push(observed.copied);
    }
    assert_eq!(copies, [[0; 4]; 2], "finish must move each allocation");
}

#[test]
fn nested_loops_retain_only_the_ordinary_and_outermost_presence_intervals() {
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
    let ids = ledger::ledger(&[
        "application .",
        "product R",
        "field R.value",
        "root r",
        "key r.id",
    ]);
    let input = project_capture::project_with_ids(&[("src/main.mw", source)], Some(&ids));
    OBSERVED.set(Some(Observed::default()));
    let result = crate::compile(&input);
    let observed = OBSERVED.take().expect("observation enabled");
    let Err(crate::CompileFailure::Diagnostics(rows)) = result else {
        panic!("the overlapping erased intervals must reject the write: {result:?}");
    };
    let start_byte = source.find("p.value").expect("the protected write exists");
    let write_span = SourceSpan {
        start_byte,
        end_byte: start_byte + "p.value".len(),
        line: 16,
        column: 25,
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
    assert_eq!(observed.functions, [4, 0, 0, 0]);
    assert_eq!(observed.calls, [5, 0, 0, 0]);
    assert_eq!(observed.copied, [0; 4]);
    // The ordinary use includes its RHS call. The while region starts before
    // its condition and includes the tail eraser; neither nested range loop
    // contributes another interval for the same older proof.
    assert_eq!(
        observed.intervals,
        vec![(write_span, 0..4), (write_span, 1..5)],
    );
}
