//! The outer-panic restoration red: a subprocess mutates every owner inside an armed
//! transaction, raises a panic that unwinds through the armed guard, catches it outside,
//! and proves byte-exact restoration plus a successful process exit. A rollback panic
//! during the unwind would abort the subprocess and fail the outer half; production
//! itself uses no `catch_unwind` — the catch here is the test's own harness.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;

use marrow_image::{
    CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef,
    FunctionDef, ImageDraft, ImageType, Instr, LedgerIdBytes, RootOccurrenceDef, Scalar,
    SemanticTarget, VariantDef,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];

/// Mutate every owner the transaction covers: both interned pools and their indexes, a
/// reserved-then-filled record and enum, a collection, the value-shape arena, a Product
/// declaration, a root occurrence, a requested site, and a function, export, and test
/// entry spending that site — seeded by `seed` so two passes touch distinct rows.
fn mutate_every_owner(txn: &mut DraftTxn<'_>, seed: u8) {
    let name = txn
        .intern_string(&format!("n{seed}"))
        .expect("a within-domain mint");
    let source = txn
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    txn.intern_int(i64::from(seed))
        .expect("a within-domain mint");
    txn.intern_text(&format!("t{seed}"))
        .expect("a within-domain mint");
    txn.intern_date(i32::from(seed))
        .expect("a within-domain mint");
    let record = txn.reserve_record_type(name).expect("a within-domain mint");
    txn.set_record_fields(
        record,
        vec![FieldDef {
            name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    )
    .expect("a reserved row fills once");
    let enum_id = txn.reserve_enum_type(name).expect("a within-domain mint");
    txn.set_enum_variants(
        enum_id,
        vec![VariantDef {
            name,
            category: false,
            payload: Vec::new(),
        }],
    )
    .expect("a reserved row fills once");
    txn.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    })
    .expect("a within-domain mint");
    txn.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let value = txn.value_scalar(Scalar::Int);
    let mut product = PRODUCT_ID;
    product[1] = seed;
    let mut field = [0x50u8; 16];
    field[0] = seed;
    txn.declare_product(
        &admitted_plan(),
        LedgerIdBytes::from_bytes(product),
        record,
        vec![DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: LedgerIdBytes::from_bytes(field),
                required: true,
                value,
            },
        }],
    )
    .expect("a well-formed declaration");
    let root = txn
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(product),
            RootOccurrenceDef {
                name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes([seed; 16]),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let handle = txn
        .bind_occurrence_site(
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root admits a whole-payload site");
    let site = txn.request_site(&handle).expect("the binding is live");
    let func = txn
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::DurExists(site), Instr::Return],
            spans: Vec::new(),
        })
        .expect("the site ref is live");
    txn.add_export(ExportId::of_local("m", &format!("n{seed}")), func);
    // A test entry's function is never also exported, so the entry gets its own body.
    let test_name = txn
        .intern_string(&format!("t_n{seed}"))
        .expect("a within-domain mint");
    let test_func = txn
        .add_function(FunctionDef {
            name: test_name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::Return],
            spans: Vec::new(),
        })
        .expect("a siteless body appends");
    txn.add_test_entry(test_name, test_func);
}

/// The subprocess half: every owner is mutated under an armed guard, an outer panic
/// unwinds through it and is caught outside, and the draft's bytes are exactly the
/// committed baseline. Run only by the outer test — a rollback panic here would abort
/// the process and fail the outer half's exit-status assertion.
#[test]
#[ignore = "the subprocess half of the outer-panic restoration red"]
fn inner_outer_panic_restores_every_owner() {
    let mut owner = ImageDraft::new();
    let mut txn = admitted(&mut owner);
    mutate_every_owner(&mut txn, 0x21);
    txn.commit();
    let baseline = owner.encode().expect("the committed draft encodes").bytes;

    let unwound = catch_unwind(AssertUnwindSafe(|| {
        let mut txn = admitted(&mut owner);
        mutate_every_owner(&mut txn, 0x22);
        panic!("the outer panic after mutating every owner");
    }));
    assert!(unwound.is_err(), "the outer panic reached the catch");
    assert_eq!(
        owner.encode().expect("the restored draft encodes").bytes,
        baseline,
        "the armed guard restored every owner byte for byte during the unwind",
    );
}

/// The outer half: run the inner case in a subprocess and require both the restoration
/// assertions and a successful process exit, so a rollback panic or double-panic abort
/// during the unwind fails this test rather than vanishing into a harness crash.
#[test]
fn an_outer_panic_subprocess_restores_and_exits_cleanly() {
    let output = Command::new(std::env::current_exe().expect("the test binary's own path"))
        .args([
            "--exact",
            "inner_outer_panic_restores_every_owner",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .expect("spawn the subprocess half");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the subprocess exited cleanly (no rollback panic or abort): {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.contains("1 passed"),
        "the subprocess ran and passed the restoration case: {stdout}",
    );
}
