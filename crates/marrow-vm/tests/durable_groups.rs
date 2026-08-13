//! GR01 exit-gate evidence: the VM stores and reads a group-bearing entry and drives
//! whole-group read/replace/erase over a real ephemeral store, built with
//! `ImageDraft → encode → verify` and no compiler dependency.
//!
//! The graph is `^books(id:int): Book { title:string required; details { pages:int } }`,
//! a keyed root with one top-level field and one root-level unkeyed `group` of one sparse
//! leaf. The tests prove: a whole-entry create carries the group and a whole-entry read
//! joins it back; a whole-group read yields the group's own record; a whole-group replace
//! obeys the group-scoped payload-only law (it changes the group and leaves the sibling
//! top-level field untouched); and a whole-group erase clears only the group's leaves,
//! leaving the entry and its top-level field present.
//!
//! Every site named here is minted through the construction seam's bind-then-request
//! protocol. That protocol has exactly one owner in the workspace and is included here
//! rather than copied.

use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SemanticTarget, SpanEntry,
};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

#[path = "../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;
use site_seam::site;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const ROOT_PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
const ROOT_PRODUCT_ID: [u8; 16] = [0x0d; 16];
const TITLE_FIELD_ID: [u8; 16] = [0x0e; 16];
const GROUP_ID: [u8; 16] = [0x20; 16];
const PAGES_FIELD_ID: [u8; 16] = [0x21; 16];

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

fn export_id(name: &str) -> ExportId {
    ExportId::of_local("", name)
}

/// Build the group-bearing image with its five exports: `seed` (create book 1 carrying
/// `details{pages:384}`), `readEntry`/`readGroup` reads, and the mutating `replaceGroup`
/// and `eraseGroup`.
fn groups_image() -> VerifiedImage {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner
        .begin_transaction(draft_owner.savepoint())
        .expect("a fresh savepoint admits");

    // The `details` group's own leaf record (one sparse `pages`), then the `Book` record
    // whose trailing group slot references it.
    let details_name = draft
        .intern_string("Book.details")
        .expect("a within-domain mint");
    let pages = draft.intern_string("pages").expect("a within-domain mint");
    let details_record = draft
        .add_record_type(RecordTypeDef {
            name: details_name,
            fields: vec![FieldDef {
                name: pages,
                ty: ImageType::scalar(Scalar::Int),
                required: false,
            }],
        })
        .expect("a within-domain mint");
    let book = draft.intern_string("Book").expect("a within-domain mint");
    let title = draft.intern_string("title").expect("a within-domain mint");
    let details = draft
        .intern_string("details")
        .expect("a within-domain mint");
    let book_record = draft
        .add_record_type(RecordTypeDef {
            name: book,
            fields: vec![
                FieldDef {
                    name: title,
                    ty: ImageType::scalar(Scalar::Text),
                    required: true,
                },
                FieldDef {
                    name: details,
                    ty: ImageType::Record {
                        idx: details_record,
                        optional: false,
                    },
                    required: true,
                },
            ],
        })
        .expect("a within-domain mint");

    let root = draft.intern_string("books").expect("a within-domain mint");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let product = LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID);
    let text_value = draft.value_scalar(Scalar::Text);
    let int_value = draft.value_scalar(Scalar::Int);
    draft
        .declare_product(
            &admitted_plan(),
            product,
            book_record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(TITLE_FIELD_ID),
                        required: true,
                        value: text_value,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes(GROUP_ID),
                    },
                },
                DeclarationMemberDef {
                    parent: Some(1),
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(PAGES_FIELD_ID),
                        required: false,
                        value: int_value,
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    let books = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");

    let members = draft.product_members(product).expect("declared");
    let root_entry = site(
        &mut draft,
        books.occurrence(),
        books.placement_path(),
        SemanticTarget::WholePayload,
    );
    let group_entry = site(
        &mut draft,
        books.occurrence(),
        members[1].path(),
        SemanticTarget::GroupEntry,
    );

    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let book_ty_idx = book_record;
    let details_ty_idx = details_record;

    // seed(): create book 1 with title "hi" and details{pages:384}.
    {
        let name = draft.intern_string("seed").expect("a within-domain mint");
        let key = draft.intern_int(1).expect("a within-domain mint");
        let title_const = draft.intern_text("hi").expect("a within-domain mint");
        let pages_const = draft.intern_int(384).expect("a within-domain mint");
        let code = vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key),         // root key
            Instr::ConstLoad(title_const), // Book.title
            Instr::ConstLoad(pages_const), // details.pages value
            Instr::SomeWrap,               // -> Some(384) for the sparse leaf
            Instr::RecordNew(details_record),
            Instr::RecordNew(book_record),
            Instr::DurCreateEntry(root_entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ];
        let func = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: spans(&code),
                code,
            })
            .expect("every site operand is live");
        draft.add_export(export_id("seed"), func);
    }

    // readEntry(): read the whole ^books[1] entry.
    add_read(
        &mut draft,
        src,
        "readEntry",
        Instr::DurReadEntry(root_entry.clone()),
        book_ty_idx,
    );
    // readGroup(): read ^books[1].details as a unit.
    add_read(
        &mut draft,
        src,
        "readGroup",
        Instr::DurReadGroup(group_entry.clone()),
        details_ty_idx,
    );

    // replaceGroup(): replace ^books[1].details with {pages:500}.
    {
        let name = draft
            .intern_string("replaceGroup")
            .expect("a within-domain mint");
        let key = draft.intern_int(1).expect("a within-domain mint");
        let pages_const = draft.intern_int(500).expect("a within-domain mint");
        let code = vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key),
            Instr::ConstLoad(pages_const),
            Instr::SomeWrap,
            Instr::RecordNew(details_record),
            Instr::DurReplaceGroup(group_entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ];
        let func = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: spans(&code),
                code,
            })
            .expect("every site operand is live");
        draft.add_export(export_id("replaceGroup"), func);
    }

    // eraseGroup(): erase ^books[1].details (clears the group's leaves).
    {
        let name = draft
            .intern_string("eraseGroup")
            .expect("a within-domain mint");
        let key = draft.intern_int(1).expect("a within-domain mint");
        let code = vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key),
            Instr::DurEraseGroup(group_entry),
            Instr::TxnCommit,
            Instr::Return,
        ];
        let func = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: spans(&code),
                code,
            })
            .expect("every site operand is live");
        draft.add_export(export_id("eraseGroup"), func);
    }

    verify(&draft.encode().expect("encode").bytes).expect("image verifies")
}

/// Add a read-only export that pushes the root key `1`, runs `op`, and returns the
/// optional record it leaves on the stack.
fn add_read(
    draft: &mut DraftTxn<'_>,
    src: marrow_image::StrId,
    name: &str,
    op: Instr,
    record: marrow_image::TypeId,
) {
    let name_id = draft.intern_string(name).expect("a within-domain mint");
    let key = draft.intern_int(1).expect("a within-domain mint");
    let code = vec![Instr::ConstLoad(key), op, Instr::Return];
    let ret = ImageType::Record {
        idx: record,
        optional: true,
    };
    let func = draft
        .add_function(FunctionDef {
            name: name_id,
            source: src,
            params: Vec::new(),
            ret,
            local_count: 0,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(export_id(name), func);
}

fn seeded(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    let Ephemeral::Ready(mut attachment) = mint_ephemeral(image) else {
        panic!("the groups image is flat-executable");
    };
    let seed = image.export_by_id(export_id("seed")).expect("seed export");
    match run_export(image, &mut attachment, seed, Vec::new()) {
        DurableRun::Ran(Ok(_)) => {}
        other => panic!("seed did not run: {}", describe(&other)),
    }
    *attachment
}

fn describe(run: &DurableRun) -> String {
    match run {
        DurableRun::Ran(Ok(_)) => "ran".to_string(),
        DurableRun::Ran(Err(fault)) => format!("fault {}", fault.code()),
        DurableRun::Parked => "parked".to_string(),
        DurableRun::Failed(code) => format!("failed {code}"),
    }
}

fn run_mut(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
) {
    let export = image.export_by_id(export_id(name)).expect("export present");
    match run_export(image, attachment, export, Vec::new()) {
        DurableRun::Ran(Ok(_)) => {}
        other => panic!("{name} did not run: {}", describe(&other)),
    }
}

fn run_read(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
) -> Value {
    let export = image.export_by_id(export_id(name)).expect("export present");
    match run_export(image, attachment, export, Vec::new()) {
        DurableRun::Ran(Ok(Some(value))) => value,
        other => panic!("{name} did not produce a value: {}", describe(&other)),
    }
}

/// Unwrap `Optional(Some(x))`.
fn present(value: Value) -> Value {
    match value {
        Value::Optional(Some(inner)) => *inner,
        other => panic!("expected a present optional, got {other:?}"),
    }
}

fn record_slots(value: Value) -> Vec<Option<Value>> {
    match value {
        Value::Record(_, slots) => slots.into_vec(),
        other => panic!("expected a record, got {other:?}"),
    }
}

fn text_of(value: Value) -> String {
    match value {
        Value::Text(s) => s.to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

/// The value of a sparse `int` leaf, read from its record slot: a present slot holds the
/// bare int; a vacant slot is `None`.
fn opt_int(slot: Option<Value>) -> Option<i64> {
    match slot {
        Some(Value::Int(v)) => Some(v),
        None => None,
        other => panic!("expected an int leaf slot, got {other:?}"),
    }
}

#[test]
fn a_whole_entry_read_joins_the_group_it_was_created_with() {
    let image = groups_image();
    let mut attachment = seeded(&image);
    let book = record_slots(present(run_read(&image, &mut attachment, "readEntry")));
    assert_eq!(text_of(book[0].clone().expect("title present")), "hi");
    let details = record_slots(book[1].clone().expect("details group present"));
    assert_eq!(
        opt_int(details.into_iter().next().expect("pages slot")),
        Some(384)
    );
}

#[test]
fn a_whole_group_read_yields_the_groups_own_record() {
    let image = groups_image();
    let mut attachment = seeded(&image);
    let details = record_slots(present(run_read(&image, &mut attachment, "readGroup")));
    assert_eq!(
        opt_int(details.into_iter().next().expect("pages slot")),
        Some(384)
    );
}

#[test]
fn a_whole_group_replace_changes_the_group_and_leaves_the_top_level_field_untouched() {
    let image = groups_image();
    let mut attachment = seeded(&image);
    run_mut(&image, &mut attachment, "replaceGroup");
    // The group changed to pages=500.
    let details = record_slots(present(run_read(&image, &mut attachment, "readGroup")));
    assert_eq!(
        opt_int(details.into_iter().next().expect("pages slot")),
        Some(500)
    );
    // The sibling top-level field is untouched (group-scoped payload-only law).
    let book = record_slots(present(run_read(&image, &mut attachment, "readEntry")));
    assert_eq!(text_of(book[0].clone().expect("title present")), "hi");
    let joined = record_slots(book[1].clone().expect("details present"));
    assert_eq!(
        opt_int(joined.into_iter().next().expect("pages slot")),
        Some(500)
    );
}

#[test]
fn a_whole_group_erase_clears_the_leaves_and_keeps_the_entry_and_top_level_field() {
    let image = groups_image();
    let mut attachment = seeded(&image);
    run_mut(&image, &mut attachment, "eraseGroup");
    // The group's leaf is now vacant, but the group (and entry) remain present.
    let details = record_slots(present(run_read(&image, &mut attachment, "readGroup")));
    assert_eq!(
        opt_int(details.into_iter().next().expect("pages slot")),
        None
    );
    let book = record_slots(present(run_read(&image, &mut attachment, "readEntry")));
    assert_eq!(text_of(book[0].clone().expect("title present")), "hi");
}
