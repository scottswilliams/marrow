use super::{BodyKind, Instr, LowerMode};
use std::cell::RefCell;

#[derive(Debug, Default)]
struct Observed {
    // Ordinary, concrete generic, proof-only, and test bodies, in that order.
    functions: [usize; 4],
    instructions: [usize; 4],
    copied: [usize; 4],
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
