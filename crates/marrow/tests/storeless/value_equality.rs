//! End-to-end value-equality tests (C02 V6): `==`/`!=` over the C02 value domain
//! — nominals, `Option`, `Result`, and user `enum`s — travel the real production
//! path through the built binary via the `value_equality` conformance fixture. The
//! VM's `Eq*` opcodes agree with the kernel's `value_equality` owner; that
//! agreement is pinned in `marrow-vm`'s `equality_agreement` test, and these cases
//! exercise the language-level verdicts.

use std::path::{Path, PathBuf};
use std::process::Command;

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/value_equality")
}

#[test]
fn value_equality_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
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
