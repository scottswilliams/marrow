//! Exit-gate evidence for bounded nested traversal (E04): the VM drives the
//! freeze-then-run `DurIterateBounded` opcode over a real ephemeral store, built with
//! `ImageDraft → encode → verify` and no compiler dependency. Each test seeds a store,
//! runs a read-only iterate export, and inspects the frozen `List[K]` and the on-more
//! `Bool` the opcode leaves on the stack.
//!
//! The kernel's 53 acquisition-law tests own the seek/skip behavior; these tests own
//! the VM's consumption of it: the frozen list is built in ascending order, the
//! on-more bit is reported, an inclusive `from` seeks, a branch site scopes to its
//! parent entry, and the frozen list obeys the single collection aggregate ceiling.

use marrow_image::{
    CollectionTypeDef, DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity,
    Scalar, SemanticPath, SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::durable::{CommitResult, DemandCoverage, Durable, EntryValue, InvocationGrant};
use marrow_kernel::equality::ValueDomain;
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// The tracer graph's fixed ledger ids.
const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const ROOT_PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
const ROOT_PRODUCT_ID: [u8; 16] = [0x0d; 16];
const TITLE_FIELD_ID: [u8; 16] = [0x0e; 16];
const BRANCH_PLACEMENT_ID: [u8; 16] = [0x30; 16];
const BRANCH_KEY_ID: [u8; 16] = [0x31; 16];
const TEXT_FIELD_ID: [u8; 16] = [0x32; 16];

fn root_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
        ),
    ])
}

fn branch_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(BRANCH_PLACEMENT_ID),
        ),
    ])
}

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

/// Build the `^books(id: int): Book { title: string required }` graph with a
/// single-level `notes(pos: int): Note { text: string required }` branch, a mutating
/// `seed` export that creates three books (ids 1,2,3) and two notes under book 1
/// (positions 10,20), and the read-only iterate exports the tests drive. The root and
/// branch keys are both `int`, so the frozen key list is `List[int]`.
fn traversal_image() -> VerifiedImage {
    let mut draft = ImageDraft::new();

    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let book_record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let note = draft.intern_string("Note");
    let text = draft.intern_string("text");
    let note_record = draft.add_record_type(RecordTypeDef {
        name: note,
        fields: vec![FieldDef {
            name: text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });

    let root = draft.intern_string("books");
    let notes = draft.intern_string("notes");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
        }],
        record: book_record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(TITLE_FIELD_ID),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes(BRANCH_PLACEMENT_ID),
                    name: notes,
                    record: note_record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: LedgerIdBytes::from_bytes(BRANCH_KEY_ID),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes(TEXT_FIELD_ID),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Text),
                    }],
                },
            ],
        },
    });

    let root_entry = draft.add_site(SiteDef::whole_payload(root_path())).index();
    let branch_entry = draft
        .add_site(SiteDef::whole_payload(branch_path()))
        .index();
    let list_int = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .index();

    let src = draft.intern_string("src/main.mw");

    // The mutating seed: create three books and two notes under book 1. Its write
    // demand widens the attachment ceiling so the store can be populated.
    {
        let name = draft.intern_string("seed");
        let title_const = draft.intern_text("t");
        let mut code = vec![Instr::TxnBegin];
        for id in [1i64, 2, 3] {
            let key = draft.intern_int(id);
            code.push(Instr::ConstLoad(key.index())); // root key
            code.push(Instr::ConstLoad(title_const.index())); // title (a string const)
            code.push(Instr::RecordNew(book_record.index()));
            code.push(Instr::DurCreateEntry(root_entry));
        }
        let book_one = draft.intern_int(1);
        for pos in [10i64, 20] {
            let branch_key = draft.intern_int(pos);
            code.push(Instr::ConstLoad(book_one.index())); // root key
            code.push(Instr::ConstLoad(branch_key.index())); // branch key
            code.push(Instr::ConstLoad(title_const.index())); // text
            code.push(Instr::RecordNew(note_record.index()));
            code.push(Instr::DurCreateEntry(branch_entry));
        }
        code.push(Instr::TxnCommit);
        code.push(Instr::Return);
        let func = draft.add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_export(export_id("seed"), func);
    }

    let list_ret = ImageType::Collection {
        idx: list_int,
        optional: false,
    };

    // A read-only export returning the frozen `List[int]` of the traversal over `site`
    // with the given `limit`. `ancestor` names the local slot holding the parent key a
    // branch site pops (root site: none). `from` reads the from key from the first
    // param.
    let add_keys_export =
        |draft: &mut ImageDraft, name: &str, site: u16, limit: u32, from: bool, ancestor: bool| {
            let name_id = draft.intern_string(name);
            let mut code = Vec::new();
            let params = if from || ancestor {
                vec![ImageType::scalar(Scalar::Int)]
            } else {
                Vec::new()
            };
            if from || ancestor {
                code.push(Instr::LocalGet(0));
            }
            code.push(Instr::DurIterateBounded {
                site,
                limit,
                from,
                list_ty: list_int,
            });
            code.push(Instr::Pop); // discard the on-more Bool; the list is returned
            code.push(Instr::Return);
            let local_count = params.len() as u16;
            let func = draft.add_function(FunctionDef {
                name: name_id,
                source: src,
                params,
                ret: list_ret,
                local_count,
                spans: spans(&code),
                code,
            });
            draft.add_export(export_id(name), func);
        };

    add_keys_export(&mut draft, "rootKeys2", root_entry, 2, false, false);
    add_keys_export(&mut draft, "rootKeys5", root_entry, 5, false, false);
    add_keys_export(&mut draft, "rootFrom", root_entry, 5, true, false);
    add_keys_export(&mut draft, "branchKeys", branch_entry, 5, false, true);

    // A read-only export returning the on-more `Bool` of the traversal over `site`.
    let add_more_export =
        |draft: &mut ImageDraft, name: &str, site: u16, limit: u32, ancestor: bool| {
            let name_id = draft.intern_string(name);
            let mut code = Vec::new();
            let params = if ancestor {
                vec![ImageType::scalar(Scalar::Int)]
            } else {
                Vec::new()
            };
            let bool_slot = params.len() as u16;
            if ancestor {
                code.push(Instr::LocalGet(0));
            }
            code.push(Instr::DurIterateBounded {
                site,
                limit,
                from: false,
                list_ty: list_int,
            });
            code.push(Instr::LocalSet(bool_slot)); // stash the on-more Bool
            code.push(Instr::Pop); // discard the frozen list
            code.push(Instr::LocalGet(bool_slot));
            code.push(Instr::Return);
            let func = draft.add_function(FunctionDef {
                name: name_id,
                source: src,
                params,
                ret: ImageType::scalar(Scalar::Bool),
                local_count: bool_slot + 1,
                spans: spans(&code),
                code,
            });
            draft.add_export(export_id(name), func);
        };

    add_more_export(&mut draft, "rootMore2", root_entry, 2, false);
    add_more_export(&mut draft, "rootMore5", root_entry, 5, false);
    add_more_export(&mut draft, "branchMore", branch_entry, 5, true);

    verify(&draft.encode().expect("encode").bytes).expect("image verifies")
}

/// Mint a fresh attachment and run the seed export against it, leaving a populated
/// store the caller reads.
fn seeded_attachment(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    let Ephemeral::Ready(mut attachment) = mint_ephemeral(image) else {
        panic!("the traversal image is flat-executable");
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

/// Run a read-only export and return its VM value.
fn run_read(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Value {
    let export = image.export_by_id(export_id(name)).expect("export present");
    match run_export(image, attachment, export, args) {
        DurableRun::Ran(Ok(Some(value))) => value,
        other => panic!("{name} did not produce a value: {}", describe(&other)),
    }
}

fn int_list(value: Value) -> Vec<i64> {
    match value {
        Value::List(_, _, items) => items
            .iter()
            .map(|item| match item {
                Value::Int(v) => *v,
                other => panic!("a frozen key is not an int: {other:?}"),
            })
            .collect(),
        other => panic!("not a list: {other:?}"),
    }
}

fn as_bool(value: Value) -> bool {
    match value {
        Value::Bool(v) => v,
        other => panic!("not a bool: {other:?}"),
    }
}

#[test]
fn a_root_traversal_freezes_keys_in_ascending_order_and_reports_more() {
    let image = traversal_image();
    let mut attachment = seeded_attachment(&image);

    // `at most 2` over three books freezes the first two ascending keys and reports a
    // further key existed.
    assert_eq!(
        int_list(run_read(&image, &mut attachment, "rootKeys2", vec![])),
        vec![1, 2]
    );
    assert!(as_bool(run_read(
        &image,
        &mut attachment,
        "rootMore2",
        vec![]
    )));

    // `at most 5` freezes all three and reports no further key.
    assert_eq!(
        int_list(run_read(&image, &mut attachment, "rootKeys5", vec![])),
        vec![1, 2, 3]
    );
    assert!(!as_bool(run_read(
        &image,
        &mut attachment,
        "rootMore5",
        vec![]
    )));
}

#[test]
fn a_root_traversal_from_seeks_inclusive() {
    let image = traversal_image();
    let mut attachment = seeded_attachment(&image);

    // The inclusive `from` key starts the walk at or after it.
    assert_eq!(
        int_list(run_read(
            &image,
            &mut attachment,
            "rootFrom",
            vec![Value::Int(2)]
        )),
        vec![2, 3]
    );
    assert_eq!(
        int_list(run_read(
            &image,
            &mut attachment,
            "rootFrom",
            vec![Value::Int(3)]
        )),
        vec![3]
    );
    // A `from` past the last key yields no keys.
    assert_eq!(
        int_list(run_read(
            &image,
            &mut attachment,
            "rootFrom",
            vec![Value::Int(4)]
        )),
        Vec::<i64>::new()
    );
}

#[test]
fn a_branch_traversal_scopes_to_its_parent_entry() {
    let image = traversal_image();
    let mut attachment = seeded_attachment(&image);

    // Book 1 carries two notes; the branch traversal under it yields exactly them.
    assert_eq!(
        int_list(run_read(
            &image,
            &mut attachment,
            "branchKeys",
            vec![Value::Int(1)]
        )),
        vec![10, 20]
    );
    assert!(!as_bool(run_read(
        &image,
        &mut attachment,
        "branchMore",
        vec![Value::Int(1)]
    )));

    // Book 2 carries no notes; its branch layer is empty.
    assert_eq!(
        int_list(run_read(
            &image,
            &mut attachment,
            "branchKeys",
            vec![Value::Int(2)]
        )),
        Vec::<i64>::new()
    );
    assert!(!as_bool(run_read(
        &image,
        &mut attachment,
        "branchMore",
        vec![Value::Int(2)]
    )));
}

/// Build a `^big(k: string): Rec { v: int required }` graph whose only read export
/// freezes up to `MAX_TRAVERSAL_BOUND` root keys, plus a one-entry `seed` export whose
/// write demand widens the attachment ceiling. The bulk of the store is seeded
/// directly through the kernel session so the frozen list can cross the aggregate
/// ceiling.
fn wide_key_image() -> (VerifiedImage, u16) {
    let mut draft = ImageDraft::new();
    let rec = draft.intern_string("Rec");
    let v = draft.intern_string("v");
    let record = draft.add_record_type(RecordTypeDef {
        name: rec,
        fields: vec![FieldDef {
            name: v,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let root = draft.intern_string("big");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(TITLE_FIELD_ID),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    let root_entry = draft.add_site(SiteDef::whole_payload(root_path())).index();
    let list_str = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");

    // A one-entry mutating seed: its presence gives the program write demand so the
    // ceiling admits the direct bulk seeding below.
    {
        let name = draft.intern_string("seed");
        let key = draft.intern_text("k");
        let value = draft.intern_int(0);
        let code = vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key.index()),
            Instr::ConstLoad(value.index()),
            Instr::RecordNew(record.index()),
            Instr::DurCreateEntry(root_entry),
            Instr::TxnCommit,
            Instr::Return,
        ];
        let func = draft.add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_export(export_id("seed"), func);
    }

    // The read export freezes up to MAX_TRAVERSAL_BOUND keys and returns the list.
    {
        let name = draft.intern_string("all");
        let code = vec![
            Instr::DurIterateBounded {
                site: root_entry,
                limit: marrow_image::bounds::MAX_TRAVERSAL_BOUND,
                from: false,
                list_ty: list_str,
            },
            Instr::Pop,
            Instr::Return,
        ];
        let func = draft.add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::Collection {
                idx: list_str,
                optional: false,
            },
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_export(export_id("all"), func);
    }

    let image = verify(&draft.encode().expect("encode").bytes).expect("image verifies");
    (image, root_entry)
}

#[test]
fn a_frozen_list_that_exceeds_the_aggregate_ceiling_faults() {
    let (image, root_entry) = wide_key_image();
    let Ephemeral::Ready(mut attachment) = mint_ephemeral(&image) else {
        panic!("the wide-key image is flat-executable");
    };

    // Seed keys whose total size crosses `MAX_AGGREGATE_BYTES` (1 MiB). Each key is
    // 3000 bytes; 400 of them measure 1.2 MB of frozen-list value, above the ceiling.
    let key_len = 3000usize;
    let count = 400usize;
    {
        let write = DemandCoverage {
            read: true,
            write: true,
        };
        let mut txn = attachment
            .txn_session(InvocationGrant::full_store(), write)
            .expect("txn session");
        let entry = txn.site(root_entry);
        for i in 0..count {
            let key = format!("{i:0width$}", width = key_len);
            debug_assert_eq!(key.len(), key_len);
            txn.create_entry(
                &entry,
                &[KeyScalar::Str(key)],
                EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(0)))],
                },
            )
            .expect("create");
        }
        assert_eq!(txn.commit(), CommitResult::Committed);
    }

    // The kernel freezes every key (well under `MAX_TRAVERSAL_BOUND`), but the VM
    // materializes them into one ordinary `List[string]`, which crosses the single
    // collection aggregate ceiling and faults — the same fault a batch `split` result
    // of this size raises, not a traversal-specific bound.
    let all = image.export_by_id(export_id("all")).expect("all export");
    match run_export(&image, &mut attachment, all, Vec::new()) {
        DurableRun::Ran(Err(fault)) => assert_eq!(fault.code(), "run.collection_limit"),
        other => panic!(
            "expected a collection-limit fault, got {}",
            describe(&other)
        ),
    }
}
