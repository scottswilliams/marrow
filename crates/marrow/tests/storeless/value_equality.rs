//! End-to-end value-equality tests: `==`/`!=` over the value domain — nominals,
//! `Option`, `Result`, and user `enum`s — travel the real production
//! path through the built binary via the `value_equality` conformance fixture. The
//! VM's `Eq*` opcodes agree with the kernel's `value_equality` owner; that
//! agreement is pinned in `marrow-vm`'s `equality_agreement` test, and these cases
//! exercise the language-level verdicts.

use crate::common::{conformance_dir, marrow_in};

#[test]
fn value_equality_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(
        &conformance_dir("value_equality"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "value_equality fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":7"#), "{summary}");
}
