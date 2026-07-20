//! Multiple resource record types per project (MR01 step 3a): the checker admits
//! more than one `resource` declaration, each becoming its own value record type,
//! while two resources sharing a name are a precise typed `check.type` rejection.
//! The durable graph still admits a single store this step; a second resource is a
//! value type, not a second root.

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// The identity ledger for a single `^ledgers` store over the `Ledger` resource. A
/// second, storeless resource needs no durable identity — it is a value type only.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Ledger 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Ledger.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root ledgers 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key ledgers.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

fn project(source: &str, ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, ids, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Two resources — one backing a store, one storeless — both usable as by-value
/// record types compile to a canonical image. Before step 3a the second resource
/// was a `check.unsupported` rejection ("only one resource type per project").
#[test]
fn two_resources_compile_together() {
    let source = r#"module main

resource Account {
    required name: string
}

resource Ledger {
    required title: string
}

store ^ledgers[id: int]: Ledger

pub fn make(): string {
    const a = Account(name: "x")
    const l = Ledger(title: "y")
    return a.name
}
"#;
    compile(&project(source, Some(IDS.as_bytes()))).unwrap_or_else(|diagnostics| {
        panic!("expected two resources to compile, got {diagnostics:#?}");
    });
}

/// Two resources sharing a name have no unambiguous record identity: the duplicate
/// is a precise typed `check.type` rejection carrying a located span.
#[test]
fn a_duplicate_resource_name_is_rejected() {
    let source = r#"module main

resource Thing {
    required a: string
}

resource Thing {
    required b: string
}

pub fn make(): string {
    return "x"
}
"#;
    let diagnostics = match compile(&project(source, None)) {
        Ok(_) => panic!("a duplicate resource name must be rejected"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics,
        Err(CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    };
    let hit = diagnostics
        .iter()
        .find(|d| d.code == "check.type")
        .unwrap_or_else(|| {
            panic!(
                "expected a `check.type` diagnostic, got {:?}",
                diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
            )
        });
    assert!(
        hit.line >= 1 && hit.column >= 1,
        "the duplicate-name rejection carries a located span: {hit:?}"
    );
}
