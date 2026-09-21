//! The verifier's machine-stack requirement, measured against the deepest image it admits.
//!
//! Verification is the trust boundary: its input is a byte string a hostile producer
//! chose, so every walk it performs must be bounded in frames as well as in work. Most
//! already are — the member walk, the type-cycle check, the flow and presence analyses all
//! drive explicit stacks — but three walks recurse natively, each bounded by a declared
//! nesting bound rather than by an explicit stack:
//!
//! | walk | bound | depth |
//! |---|---|---|
//! | `durable::members::decode_value_shape` | `MAX_DURABLE_VALUE_DEPTH` | 32 |
//! | `durable::members::value_shape_matches` | the shape that decode admitted | 32 |
//! | `durable::seal::seal_branch_run` | `MAX_DURABLE_DEPTH` | 16 |
//!
//! A frame count alone is not a stack bound, so [`VERIFY_STACK_BYTES`] states what those
//! depths are allowed to cost and this file measures it on one image that reaches all
//! three at once, along the longest path a site can name.

use marrow_image::bounds::{MAX_DURABLE_DEPTH, MAX_DURABLE_VALUE_DEPTH, MAX_SITE_PATH_STEPS};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, PlannedSiteRef, RecordTypeDef,
    RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, TypeId, ValueShapeNodeId,
};
use marrow_test_support::{admitted_plan, site};
use marrow_verify::{VERIFY_STACK_BYTES, verify};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];

fn ledger_id(base: u8, ordinal: usize) -> LedgerIdBytes {
    let mut bytes = [base; 16];
    bytes[0] = ordinal as u8;
    bytes[1] = (ordinal >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// One value-shape level: the arena node and the materialized record type a durable
/// field carrying it must declare.
#[derive(Clone, Copy)]
struct Level {
    shape: ValueShapeNodeId,
    record: TypeId,
    ty: ImageType,
}

/// A value shape at exactly [`MAX_DURABLE_VALUE_DEPTH`]: a bare scalar under
/// `MAX_DURABLE_VALUE_DEPTH - 1` single-leaf struct levels. One more level is refused, so
/// this is the deepest shape the verifier's value-shape decode and its type match can be
/// asked to follow.
fn deepest_value(draft: &mut DraftTxn<'_>) -> Level {
    let mut level = Level {
        shape: draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints"),
        record: TypeId::from_index(0),
        ty: ImageType::scalar(Scalar::Int),
    };
    for ordinal in 1..MAX_DURABLE_VALUE_DEPTH {
        let name = draft
            .intern_string(&format!("V{ordinal}"))
            .expect("a within-domain mint");
        let leaf = draft.intern_string("inner").expect("a within-domain mint");
        let record = draft
            .add_record_type(RecordTypeDef {
                name,
                fields: vec![FieldDef {
                    name: leaf,
                    ty: level.ty,
                    required: true,
                }],
            })
            .expect("a within-domain mint");
        level = Level {
            shape: draft
                .value_struct(vec![level.shape])
                .expect("a within-bounds shape appends"),
            record,
            ty: record_type(record),
        };
    }
    level
}

fn record_type(record: TypeId) -> ImageType {
    ImageType::Record {
        idx: record,
        optional: false,
    }
}

/// A record holding one field of `ty`, which is what the record↔member tie requires of
/// every container in this corpus: each declares exactly one field member.
fn holder(draft: &mut DraftTxn<'_>, name: &str, ty: ImageType) -> TypeId {
    let name = draft.intern_string(name).expect("a within-domain mint");
    let field = draft.intern_string("deep").expect("a within-domain mint");
    draft
        .add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field,
                ty,
                required: true,
            }],
        })
        .expect("a within-domain mint")
}

/// The deepest image the bounds admit: a keyed branch chain nested to
/// [`MAX_DURABLE_DEPTH`], every level carrying one durable field whose value shape is
/// itself at [`MAX_DURABLE_VALUE_DEPTH`]. The branch chain is what `seal_branch_run`
/// descends; the value shape is what `decode_value_shape` and `value_shape_matches`
/// descend; both are at their bound, in one image, on one path.
fn deepest_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();

    let deep = deepest_value(&mut draft);
    let entry = holder(&mut draft, "R", deep.ty);
    let branch_records: Vec<TypeId> = (1..MAX_DURABLE_DEPTH)
        .map(|level| holder(&mut draft, &format!("R.b{level}"), deep.ty))
        .collect();
    let branch_names: Vec<_> = (1..MAX_DURABLE_DEPTH)
        .map(|level| {
            draft
                .intern_string(&format!("b{level}"))
                .expect("a within-domain mint")
        })
        .collect();

    // Commands alternate: the field at this level, then the branch that opens the next.
    // A member's level is one past its parent's, so the last branch sits at
    // `MAX_DURABLE_DEPTH - 1` and the field it holds sits at `MAX_DURABLE_DEPTH`.
    let mut members = Vec::new();
    let mut parent = None;
    for (level, (record, name)) in branch_records.iter().zip(&branch_names).enumerate() {
        members.push(DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Field {
                id: ledger_id(0x20, level),
                required: true,
                value: deep.shape,
            },
        });
        members.push(DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Branch {
                placement: ledger_id(0x30, level),
                name: *name,
                record: *record,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: ledger_id(0x40, level),
                }],
            },
        });
        parent = Some(members.len() as u32 - 1);
    }
    members.push(DeclarationMemberDef {
        parent,
        shape: DeclarationMemberShape::Field {
            id: ledger_id(0x20, MAX_DURABLE_DEPTH),
            required: true,
            value: deep.shape,
        },
    });

    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            entry,
            members,
        )
        .expect("a well-formed declaration");
    let root_name = draft.intern_string("deep").expect("a within-domain mint");
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");

    // The deepest field's own site: the application step, the root placement step, and one
    // step per nesting level is exactly `MAX_SITE_PATH_STEPS`, so a function reading it
    // drives the code, flow, and presence phases over the longest path the image admits.
    let mut path = draft
        .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
        .expect("the Product is declared")[1]
        .path()
        .clone();
    loop {
        let members = draft
            .members_of(&path)
            .expect("the declaration row is live");
        let Some(next) = members.last() else { break };
        path = next.path().clone();
    }
    let deepest = site(
        &mut draft,
        root.occurrence(),
        &path,
        SemanticTarget::FieldLeaf,
    );
    add_reader(&mut draft, deepest, deep.record);
    draft.encode().expect("the corpus fits every bound").bytes
}

/// A function reading the deepest site, exported so the verifier's function phases reach
/// it.
fn add_reader(draft: &mut DraftTxn<'_>, site: PlannedSiteRef, read: TypeId) {
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("read").expect("a within-domain mint");
    // The site's key tuple is the root's key column plus every branch hop's, flattened, so
    // the deepest site takes one operand per nesting level.
    let mut code: Vec<Instr> = (0..MAX_DURABLE_DEPTH).map(|_| Instr::LocalGet(0)).collect();
    code.push(Instr::DurReadField(site));
    code.push(Instr::Return);
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let read = draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Record {
                idx: read,
                optional: true,
            },
            local_count: 1,
            spans,
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "read"), read);
}

/// The deepest image the bounds admit verifies inside [`VERIFY_STACK_BYTES`].
///
/// The budget is the claim; the thread is what tests it. A recursion that grew past the
/// budget would abort here rather than pass on the ambient test stack, which is 2 MiB and
/// would absorb any depth these bounds can reach.
#[test]
fn the_deepest_admitted_image_verifies_inside_the_stack_budget() {
    let image = deepest_image();
    let worker = std::thread::Builder::new()
        .stack_size(VERIFY_STACK_BYTES)
        .spawn(move || {
            let sealed = verify(&image).expect("the deepest admitted image verifies");
            assert_eq!(
                sealed.semantic_nodes().len(),
                2 * MAX_DURABLE_DEPTH,
                "the root, one field and one branch per nesting level, and the deepest field",
            );
            let deepest = sealed
                .semantic_nodes()
                .iter()
                .map(|node| node.path.steps().len())
                .max()
                .expect("the graph has nodes");
            assert_eq!(
                deepest, MAX_SITE_PATH_STEPS,
                "the corpus must reach the longest path a site can name",
            );
        })
        .expect("spawn the budget worker");
    worker.join().expect("verification completes in the budget");
}
