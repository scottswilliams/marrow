//! Slice K.4 hostile-image evidence (design §E, finding 9), two suites:
//!
//! (a) *stale-digest*: byte flips over a good image with no rehash — every one must
//!     reject at phase 1 (the digest no longer matches the payload).
//! (b) *structured single-invariant*: artifacts that each violate exactly one later
//!     invariant, carrying a valid (recomputed or encoder-computed) digest — each
//!     must reject at the phase that owns that invariant.
//!
//! Semantically valid rewrites are allowed to verify and are not asserted to reject.

use marrow_image::{
    CollectionTypeDef, DurableEnumMemberShape, DurableIndexComponent, DurableIndexShape,
    DurableMemberDef, DurableValueShape, EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity,
    Scalar, SemanticPath, SemanticStep, SemanticStepKind, SiteDef, SpanEntry, VariantDef, image_id,
};
use marrow_verify::verify;

/// The tracer graph's fixed ledger ids, shared by the durable-schema builders and
/// the site-path helpers so a hostile mutation can target one precisely.
const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const VALUE_FIELD_ID: [u8; 16] = [0x0e; 16];
const LABEL_FIELD_ID: [u8; 16] = [0x0f; 16];

/// The semantic path of the tracer root itself: `application -> placement`.
fn root_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
    ])
}

/// The semantic path of a top-level field of the tracer root.
fn field_path(field_id: [u8; 16]) -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
        SemanticStep::new(SemanticStepKind::Field, LedgerIdBytes::from_bytes(field_id)),
    ])
}

/// The tracer `Counter` record's durable member tree: `value:int` required then
/// `label:string` sparse, matching the `durable_schema` record fields so the
/// verifier's member-tree/record cross-check passes.
fn counters_members() -> Vec<DurableMemberDef> {
    vec![
        DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x0e; 16]),
            required: true,
            value: DurableValueShape::Scalar(Scalar::Int),
        },
        DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x0f; 16]),
            required: false,
            value: DurableValueShape::Scalar(Scalar::Text),
        },
    ]
}

/// A well-formed multi-function image: a caller exporting `main` that calls a helper,
/// plus a couple of constants. Every hostile case derives from this.
fn good_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let helper_name = draft.intern_string("helper");
    let seven = draft.intern_int(7);
    let helper_code = vec![Instr::ConstLoad(seven.index()), Instr::Return];
    let helper = draft.add_function(FunctionDef {
        name: helper_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&helper_code),
        code: helper_code,
    });
    let main_name = draft.intern_string("main");
    let main_code = vec![Instr::Call(helper.index()), Instr::Return];
    let main = draft.add_function(FunctionDef {
        name: main_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&main_code),
        code: main_code,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().expect("encode").bytes
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

/// Recompute and rewrite the digest over the payload (every byte after the slot).
fn rehash(bytes: &mut [u8]) {
    let id = image_id(&bytes[37..]);
    bytes[5..37].copy_from_slice(&id.0);
}

fn code_of(bytes: &[u8]) -> String {
    verify(bytes)
        .err()
        .map(|r| r.code().to_string())
        .unwrap_or_else(|| "VERIFIED".to_string())
}

/// The nine section frames as `(id, body_offset, body_len)` (header is 38 bytes).
fn sections(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut off = 38usize;
    for _ in 0..9 {
        let id = bytes[off];
        off += 1;
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        out.push((id, off, len));
        off += len;
    }
    out
}

// --- Suite (a): stale-digest, no rehash. Every case rejects at the envelope. ---

#[test]
fn stale_digest_slot_flip() {
    let mut bytes = good_image();
    bytes[10] ^= 0xFF;
    assert_eq!(code_of(&bytes), "image.envelope");
}

#[test]
fn stale_section_body_flip() {
    let mut bytes = good_image();
    // A byte well inside the section area, not rehashed.
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    assert_eq!(code_of(&bytes), "image.envelope");
}

#[test]
fn stale_truncation() {
    let mut bytes = good_image();
    bytes.truncate(bytes.len() - 3);
    assert_eq!(code_of(&bytes), "image.envelope");
}

// --- Suite (b): structured single-invariant, valid digest. ---

#[test]
fn rehashed_bad_version_rejects_at_envelope() {
    let mut bytes = good_image();
    bytes[4] = 0x01;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.envelope");
}

#[test]
fn rehashed_bad_section_count_rejects_at_envelope() {
    let mut bytes = good_image();
    bytes[37] = 6;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.envelope");
}

#[test]
fn rehashed_export_index_out_of_range_rejects_at_table() {
    let mut bytes = good_image();
    // EXPORTS is section id 6: body = count(u16), then per export id(32 bytes) func(u16).
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 6).unwrap();
    let func_field = body + 2 + 32; // after the count and the first export's id
    bytes[func_field] = 0xFF;
    bytes[func_field + 1] = 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

/// An image with two exported functions, `a` and `b`, each returning a constant.
/// The EXPORTS entries are two `32-byte id ‖ u16 func` records the encoder writes
/// in ascending id order.
fn two_export_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let one = draft.intern_int(1);
    let a_name = draft.intern_string("a");
    let a_code = vec![Instr::ConstLoad(one.index()), Instr::Return];
    let a = draft.add_function(FunctionDef {
        name: a_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&a_code),
        code: a_code,
    });
    let b_name = draft.intern_string("b");
    let b_code = vec![Instr::ConstLoad(one.index()), Instr::Return];
    let b = draft.add_function(FunctionDef {
        name: b_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&b_code),
        code: b_code,
    });
    draft.add_export(ExportId::of_local("", "a"), a);
    draft.add_export(ExportId::of_local("", "b"), b);
    draft.encode().expect("encode").bytes
}

#[test]
fn rehashed_out_of_order_export_ids_reject_at_table() {
    // Swap the two 34-byte EXPORTS entries so their ids descend. The verifier
    // requires strictly ascending ids, so the second entry is now out of order.
    let mut bytes = two_export_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 6).unwrap();
    let first = body + 2;
    let entry = 32 + 2;
    let (a, b) = (first, first + entry);
    let mut e0 = bytes[a..a + entry].to_vec();
    let mut e1 = bytes[b..b + entry].to_vec();
    std::mem::swap(&mut e0, &mut e1);
    bytes[a..a + entry].copy_from_slice(&e0);
    bytes[b..b + entry].copy_from_slice(&e1);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_two_exports_of_one_function_reject_at_table() {
    // Point the second export's function field at the first export's function, so
    // one function is the target of two exports — forbidden at v0.
    let mut bytes = two_export_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 6).unwrap();
    let first_func = body + 2 + 32;
    let second_func = first_func + 32 + 2;
    let func0 = bytes[first_func..first_func + 2].to_vec();
    bytes[second_func..second_func + 2].copy_from_slice(&func0);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_unknown_const_tag_rejects_at_table() {
    let mut bytes = good_image();
    // CONSTS is section id 4: body = count(u16), then per const tag(u8) + payload.
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 4).unwrap();
    bytes[body + 2] = 0x7F; // corrupt the first constant's tag
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn function_phase_unreachable_instruction() {
    // Built through the draft, so the digest is valid; the extra Return is dead.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let one = draft.intern_int(1);
    let code = vec![
        Instr::ConstLoad(one.index()),
        Instr::Return,
        Instr::ConstLoad(one.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

#[test]
fn function_phase_call_argument_type_mismatch() {
    // helper(n: int); main() calls it with a bool argument.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let helper_name = draft.intern_string("helper");
    let helper_code = vec![Instr::LocalGet(0), Instr::Return];
    let helper = draft.add_function(FunctionDef {
        name: helper_name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&helper_code),
        code: helper_code,
    });
    let main_name = draft.intern_string("main");
    let flag = draft.intern_bool(true);
    let main_code = vec![
        Instr::ConstLoad(flag.index()),
        Instr::Call(helper.index()),
        Instr::Return,
    ];
    let main = draft.add_function(FunctionDef {
        name: main_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&main_code),
        code: main_code,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

#[test]
fn closure_phase_mutual_recursion() {
    // ping -> pong -> ping: a two-node cycle.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let ping_name = draft.intern_string("ping");
    let pong_name = draft.intern_string("pong");
    // ping calls function index 1 (pong); pong calls index 0 (ping).
    let ping_code = vec![Instr::Call(1), Instr::Return];
    let ping = draft.add_function(FunctionDef {
        name: ping_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&ping_code),
        code: ping_code,
    });
    let pong_code = vec![Instr::Call(0), Instr::Return];
    draft.add_function(FunctionDef {
        name: pong_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&pong_code),
        code: pong_code,
    });
    draft.add_export(ExportId::of_local("", "ping"), ping);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.closure");
}

// --- Phase-5 durable transaction-flow hostiles (design §E phase 5). ---

/// Build the tracer-like durable schema into `draft`: a `Counter { value:int
/// required, label:string sparse }` at root `^counters(name:string)`, returning the
/// entry, required-field, and sparse-field site indices.
fn durable_schema(draft: &mut ImageDraft) -> (u16, u16, u16) {
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: counters_members(),
        },
    });
    let entry = draft.add_site(SiteDef::whole_payload(root_path()));
    let value_site = draft.add_site(SiteDef::field_leaf(field_path(VALUE_FIELD_ID)));
    let label_site = draft.add_site(SiteDef::field_leaf(field_path(LABEL_FIELD_ID)));
    (entry.index(), value_site.index(), label_site.index())
}

/// Encode a single mutating export `put(k:string, v:int)` whose body is `code`.
fn put_export(code: Vec<Instr>) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let (_entry, _value, _label) = durable_schema(&mut draft);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft
}

/// Encode a read-only export `read(k:string): T?` that reads the field at site index
/// `site` (one of the sites `durable_schema` registers) returning `ret`.
fn read_field_export(site: u16, ret: ImageType) -> ImageDraft {
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("read");
    let code = vec![Instr::LocalGet(0), Instr::DurReadField(site), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Text)],
        ret,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "read"), func);
    draft
}

#[test]
fn the_durable_opcode_site_determines_the_reconstructed_demand() {
    // Two read exports that differ only in the field a durable opcode reads. Demand
    // is reconstructed from the sealed site the bytecode names — there is no
    // serialized demand summary — so a change to which site an opcode reads changes
    // the reconstructed atom's path and the export's demand id.
    let value_site = {
        let mut probe = ImageDraft::new();
        durable_schema(&mut probe).1
    };
    let label_site = {
        let mut probe = ImageDraft::new();
        durable_schema(&mut probe).2
    };
    let value = read_field_export(value_site, ImageType::opt_scalar(Scalar::Int));
    let label = read_field_export(label_site, ImageType::opt_scalar(Scalar::Text));

    let value_image = verify(&value.encode().unwrap().bytes).expect("value read verifies");
    let label_image = verify(&label.encode().unwrap().bytes).expect("label read verifies");

    let value_export = &value_image.exports()[0];
    let label_export = &label_image.exports()[0];

    // Each demands a single read atom on its own field node.
    assert_eq!(value_export.demand().atoms().len(), 1);
    assert_eq!(
        *value_export.demand().atoms()[0].path().node_id().bytes(),
        VALUE_FIELD_ID
    );
    assert_eq!(
        *label_export.demand().atoms()[0].path().node_id().bytes(),
        LABEL_FIELD_ID
    );
    // Different sites read, so the demand identities differ.
    assert_ne!(value_export.demand_id(), label_export.demand_id());
}

/// Encode a read-only export whose body runs a bounded-traversal opcode over the root
/// entry site with the given `limit`/`from`, then balances the frozen `List[string]`
/// and on-more `Bool` off the stack (the export returns Unit). When `from`, a string
/// key param is pushed first as the inclusive lower bound so the head type-checks up to
/// the opcode.
fn iterate_root_export(limit: u32, from: bool) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let (entry, _value, _label) = durable_schema(&mut draft);
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("iter");
    let (params, mut code): (Vec<ImageType>, Vec<Instr>) = if from {
        (
            vec![ImageType::scalar(Scalar::Text)],
            vec![Instr::LocalGet(0)],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let local_count = params.len() as u16;
    code.push(Instr::DurIterateBounded {
        site: entry,
        limit,
        from,
        list_ty,
    });
    // Discard the on-more Bool then the frozen List so the Unit return sees an empty
    // stack; the opcode's stack effect is what these hostiles exercise.
    code.push(Instr::Pop);
    code.push(Instr::Pop);
    code.push(Instr::Return);
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params,
        ret: ImageType::Unit,
        local_count,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "iter"), func);
    draft
}

#[test]
fn a_bounded_traversal_over_a_root_verifies_and_type_checks() {
    // The freeze-then-run opcode types and executes: over the root entry family it pops
    // the inclusive `from` key when present, pushes the frozen `List[string]` and the
    // on-more `Bool`, and the image seals. Both the no-`from` and inclusive-`from` forms
    // verify.
    for from in [false, true] {
        assert_eq!(
            code_of(&iterate_root_export(2, from).encode().unwrap().bytes),
            "VERIFIED",
        );
    }
}

#[test]
fn a_bounded_traversal_over_a_branch_verifies_and_type_checks() {
    // A branch site traverses the branch family beneath a fixed root entry: the ancestor
    // root key (int) is popped, the frozen `List[string]` of branch keys and the on-more
    // `Bool` are pushed, and the image seals.
    let (mut draft, _branch_record) = flat_branch_draft();
    let site = draft.add_site(SiteDef::whole_payload(branch_entry_path()));
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("notes");
    let code = vec![
        Instr::LocalGet(0), // the root key: the ancestor locating the branch parent
        Instr::DurIterateBounded {
            site: site.index(),
            limit: 3,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "notes"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

#[test]
fn a_zero_or_oversized_traversal_bound_is_refused() {
    // The `at most N` bound is a positive compile-time constant: zero and a bound above
    // `MAX_TRAVERSAL_BOUND` are refused as out of range, so a hostile image cannot
    // smuggle an unbounded or overlarge frozen-key allocation.
    for limit in [0, marrow_image::bounds::MAX_TRAVERSAL_BOUND + 1] {
        let rejection = verify(&iterate_root_export(limit, false).encode().unwrap().bytes)
            .expect_err("an out-of-range traversal bound is refused");
        assert_eq!(rejection.code(), "image.function");
        assert_eq!(
            rejection.detail(),
            "bounded traversal bound is out of range"
        );
    }
}

#[test]
fn a_bounded_traversal_with_a_mismatched_list_type_rejects() {
    // The frozen-list COLLTYPES index must name exactly `List[K]` for the traversed key
    // `K` (here the root key is `string`). An image naming a `List[int]` is a forged
    // frozen-list type the verifier refuses before the runtime materializes it.
    let mut draft = ImageDraft::new();
    let (entry, _value, _label) = durable_schema(&mut draft);
    let wrong_list = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("iter");
    let code = vec![
        Instr::DurIterateBounded {
            site: entry,
            limit: 2,
            from: false,
            list_ty: wrong_list,
        },
        Instr::Pop,
        Instr::Pop,
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
    draft.add_export(ExportId::of_local("", "iter"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a mismatched frozen-list type is refused");
    assert_eq!(rejection.code(), "image.function");
    assert_eq!(
        rejection.detail(),
        "bounded traversal list type does not name a list of the traversed key"
    );
}

#[test]
fn a_bounded_branch_traversal_missing_its_ancestor_key_rejects() {
    // A branch traversal pops the ancestor root key locating the parent entry. Pushing
    // no ancestor key leaves that pop against an empty stack — a key-arity forgery the
    // verifier refuses.
    let (mut draft, _branch_record) = flat_branch_draft();
    let site = draft.add_site(SiteDef::whole_payload(branch_entry_path()));
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("notes");
    let code = vec![
        // No ancestor root key pushed before the opcode.
        Instr::DurIterateBounded {
            site: site.index(),
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "notes"), func);
    let rejection =
        verify(&draft.encode().unwrap().bytes).expect_err("a missing ancestor key is refused");
    assert_eq!(rejection.code(), "image.function");
    assert_eq!(rejection.detail(), "operand stack underflow");
}

#[test]
fn a_bounded_traversal_over_a_field_leaf_site_rejects() {
    // Bounded traversal iterates the layer a site's placement belongs to. A field-leaf
    // site names a single scalar leaf, not a traversable entry family, so an image aiming
    // the opcode at a field site is refused before any frozen-key allocation.
    let mut draft = ImageDraft::new();
    let (_entry, value_site, _label) = durable_schema(&mut draft);
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("iter");
    let code = vec![
        Instr::DurIterateBounded {
            site: value_site,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
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
    draft.add_export(ExportId::of_local("", "iter"), func);
    let rejection = verify(&draft.encode().unwrap().bytes)
        .expect_err("a traversal over a field-leaf site is refused");
    assert_eq!(rejection.code(), "image.function");
    assert_eq!(rejection.detail(), "operation requires an entry site");
}

#[test]
fn a_bounded_traversal_after_commit_rejects() {
    // The commit consumes the session's engine transaction, so no durable operation may
    // follow it. A bounded traversal is a durable read; the flow lattice refuses it after
    // commit exactly as it refuses a post-commit field read, so the runtime never reaches
    // a consumed transaction.
    let mut draft = ImageDraft::new();
    let (entry, value_site, _label) = durable_schema(&mut draft);
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::DurIterateBounded {
            site: entry,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    let rejection =
        verify(&draft.encode().unwrap().bytes).expect_err("a post-commit traversal is refused");
    assert_eq!(rejection.code(), "image.flow");
    assert_eq!(
        rejection.detail(),
        "a durable operation follows the transaction's commit"
    );
}

#[test]
fn a_traversal_list_type_naming_a_map_or_a_dangling_index_rejects() {
    // `list_ty` must name exactly `List[K]` for the traversed key. A COLLTYPES index that
    // names a `Map` shape, and one that dangles past the collection table, are each a
    // forged frozen-list type the verifier refuses before the runtime materializes it.
    let build = |list_ty: u16| -> Vec<u8> {
        let mut draft = ImageDraft::new();
        let (entry, _value, _label) = durable_schema(&mut draft);
        // One well-formed `Map` row at index 0: a valid collection, but the wrong kind for
        // a frozen key list. Index 1 dangles one past the single-row table.
        draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Text),
        });
        let src = draft.intern_string("src/main.mw");
        let name = draft.intern_string("iter");
        let code = vec![
            Instr::DurIterateBounded {
                site: entry,
                limit: 2,
                from: false,
                list_ty,
            },
            Instr::Pop,
            Instr::Pop,
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
        draft.add_export(ExportId::of_local("", "iter"), func);
        draft.encode().unwrap().bytes
    };
    for list_ty in [0u16, 1] {
        let rejection = verify(&build(list_ty)).expect_err("a non-List[K] frozen type is refused");
        assert_eq!(rejection.code(), "image.function");
        assert_eq!(
            rejection.detail(),
            "bounded traversal list type does not name a list of the traversed key"
        );
    }
}

#[test]
fn the_retired_next_key_opcode_byte_is_no_longer_decodable() {
    // 0x39 was the unbounded `DurNextKey` opcode, deleted with the whole family when
    // durable traversal became always-bounded. An image carrying that byte where an
    // opcode is expected — re-digested so the envelope passes — is refused as an
    // unknown opcode, so no forged image can resurrect the retired op.
    let mut bytes = iterate_root_export(2, false).encode().unwrap().bytes;
    // Locate the bounded-traversal opcode and overwrite its opcode byte with 0x39.
    let opcode = [0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00];
    let at = bytes
        .windows(opcode.len())
        .position(|w| w == opcode)
        .expect("the bounded-traversal opcode is present");
    bytes[at] = 0x39;
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("a retired opcode byte is refused");
    assert_eq!(rejection.code(), "image.function");
    assert_eq!(rejection.detail(), "unknown or not-yet-supported opcode");
}

#[test]
fn a_malformed_from_flag_byte_is_refused_at_decode() {
    // The `from` operand is a strict 0/1 flag. A hostile image that sets it to 0x02 —
    // re-digested so the envelope passes — is refused when the opcode is decoded, so no
    // third from-state can be smuggled past the bounded-traversal decoder.
    let mut bytes = iterate_root_export(2, false).encode().unwrap().bytes;
    // The encoded opcode is the ten bytes `0x3B <site:2=0> <limit:4=2> <from:1=0>
    // <list_ty:2=0>`; locate the full encoding and flip the trailing from flag to an
    // out-of-range 0x02 (this also pins the opcode's exact width).
    let opcode = [0x3B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00];
    let at = bytes
        .windows(opcode.len())
        .position(|w| w == opcode)
        .expect("the bounded-traversal opcode is present");
    assert_eq!(bytes[at + 7], 0x00, "the from flag starts cleared");
    bytes[at + 7] = 0x02;
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("a malformed from flag is refused");
    assert_eq!(rejection.code(), "image.function");
    assert_eq!(rejection.detail(), "malformed bool operand");
}

#[test]
fn durable_put_export_verifies() {
    // The well-formed baseline the flow hostiles derive from.
    let (_entry, value_site, _label) = {
        let mut probe = ImageDraft::new();
        durable_schema(&mut probe)
    };
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

/// A well-formed durable image (the tracer schema plus one verifying `put` export),
/// the baseline for the durable-contract-id hostiles.
fn good_durable_image() -> Vec<u8> {
    put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(1),
        Instr::TxnCommit,
        Instr::Return,
    ])
    .encode()
    .unwrap()
    .bytes
}

#[test]
fn rehashed_mutated_durable_contract_id_rejects_at_table() {
    // The DURABLE section (id 3) closes with the 32-byte contract id. Flipping a byte
    // of it and rehashing the envelope leaves an id the verifier's independent
    // recomputation from the decoded graph will not match.
    let mut bytes = good_durable_image();
    let (_, body, len) = *sections(&bytes).iter().find(|(id, ..)| *id == 3).unwrap();
    let id_byte = body + len - 1;
    bytes[id_byte] ^= 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_mutated_durable_field_flag_breaks_the_contract_id() {
    // Flipping the required flag of the first durable field mutates the graph the
    // verifier recomputes the contract over, so the carried id no longer matches —
    // the contract id binds the field profile, not only the root and key.
    let mut bytes = good_durable_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 2).unwrap();
    // TYPES: count(2) | type: name(2) field_count(2) | field0: name(2) tag(1) required(1)
    let required0 = body + 2 + 2 + 2 + 2 + 1;
    bytes[required0] ^= 0x01;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_mutated_ledger_id_breaks_the_contract_id() {
    // The DURABLE section opens with the root count and then the application's
    // 16-byte ledger id. Flipping one id byte mutates the graph the verifier
    // recomputes the contract over, so the carried id no longer matches — the
    // contract id binds the ledger identities, not the source names.
    let mut bytes = good_durable_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 3).unwrap();
    let application0 = body + 2;
    bytes[application0] ^= 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_duplicated_ledger_id_rejects_at_table() {
    // Entropy-minted ids are pairwise distinct by construction; overwriting the
    // root's placement id with the application id forges two equal identities in
    // one durable table and is rejected before the contract recomputation.
    let mut bytes = good_durable_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 3).unwrap();
    // DURABLE: count(2) | application(16) | root: name(2) key_count(2)
    //   [key-tag(1) key_id(16)] record(2) placement(16)…
    let application0 = body + 2;
    let placement0 = body + 2 + 16 + 2 + 2 + (1 + 16) + 2;
    let application: [u8; 16] = bytes[application0..application0 + 16].try_into().unwrap();
    bytes[placement0..placement0 + 16].copy_from_slice(&application);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_mutated_key_id_breaks_the_contract_id() {
    // A key column's ledger id travels inside the key tuple. Flipping one byte of
    // it mutates the graph the verifier recomputes the contract over, so the
    // carried id no longer matches — the contract binds each key column's identity.
    let mut bytes = good_durable_image();
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 3).unwrap();
    // Into the single key column: past count(2) application(16) name(2)
    // key_count(2) key-tag(1) to the 16-byte key id.
    let key_id0 = body + 2 + 16 + 2 + 2 + 1;
    bytes[key_id0] ^= 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn a_composite_root_write_opcode_with_a_truncated_key_path_rejects() {
    // A composite-key root is executable, addressed by its whole two-column key tuple. A
    // forged write whose body supplies the value plus only one key column — too few for
    // the two-column key-path the verifier derives from the schema — cannot satisfy the
    // operand stack, so it is refused during per-function typing (the write-path
    // counterpart of the read-path truncation hostile).
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![FieldDef {
            name: value,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let root = draft.intern_string("pairs");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x0c; 16]),
            },
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes([0x0e; 16]),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    let value_site = draft.add_site(SiteDef::field_leaf(field_path(VALUE_FIELD_ID)));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site.index()),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    // The truncated key-path is rejected during per-function structural/type recording
    // (the `image.function` phase), where the operation's key-path is typed.
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// Build a `Book { title:string required }` root at `^books(id:int)` whose durable
/// member tree adds a static `details` group (holding `pages:int`) and a keyed
/// `notes(noteId:string)` branch (holding `text:string required`). The record
/// carries only the top-level `title` field, so the verifier's member-tree/record
/// cross-check passes. When `with_site` is true, an operation site and a reading
/// function are added — the not-yet-executable shape a forged image would need.
fn group_branch_draft(with_site: bool) -> ImageDraft {
    group_branch_draft_with_branch_record(with_site, true)
}

/// As [`group_branch_draft`], but the branch's materialized record marks its `text`
/// field with `branch_record_required`. The branch *member* always marks `text`
/// required, so passing `false` builds an image whose branch record disagrees with
/// its member fields — the forgery `validate_branch_records` must reject.
fn group_branch_draft_with_branch_record(
    with_site: bool,
    branch_record_required: bool,
) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let root = draft.intern_string("books");
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Book.notes");
    let notes_text = draft.intern_string("text");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: branch_record_required,
        }],
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Group {
                    id: LedgerIdBytes::from_bytes([0x20; 16]),
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x21; 16]),
                        required: false,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    }],
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes([0x30; 16]),
                    name: notes,
                    record: notes_record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x31; 16]),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x32; 16]),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Text),
                    }],
                },
            ],
        },
    });
    let src = draft.intern_string("src/main.mw");
    if with_site {
        let site = draft.add_site(SiteDef::field_leaf(field_path(VALUE_FIELD_ID)));
        let name = draft.intern_string("read");
        let code = vec![
            Instr::LocalGet(0),
            Instr::DurReadField(site.index()),
            Instr::Return,
        ];
        let func = draft.add_function(FunctionDef {
            name,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::opt_scalar(Scalar::Text),
            local_count: 1,
            spans: spans(&code),
            code,
        });
        draft.add_export(ExportId::of_local("", "read"), func);
    } else {
        let name = draft.intern_string("label");
        let zero = draft.intern_int(0);
        let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
        let func = draft.add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_export(ExportId::of_local("", "label"), func);
    }
    draft
}

/// The fixed index ids of the indexed tracer graph.
const BY_LABEL_INDEX_ID: [u8; 16] = [0x70; 16];
const BY_VALUE_INDEX_ID: [u8; 16] = [0x71; 16];

/// The semantic path of a managed index of the tracer root: `application ->
/// placement -> index`.
fn index_path(index_id: [u8; 16]) -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
        SemanticStep::new(SemanticStepKind::Index, LedgerIdBytes::from_bytes(index_id)),
    ])
}

/// A well-formed indexed tracer graph: the counters root plus a nonunique
/// `byLabel(label, k)` and a unique `byValue(value)`. `component` overrides the first
/// index's projection, so a hostile test can point a component at a leaf the root does
/// not carry.
fn indexed_draft(by_label_components: Vec<DurableIndexComponent>) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            members: counters_members(),
            indexes: vec![
                DurableIndexShape {
                    id: LedgerIdBytes::from_bytes(BY_LABEL_INDEX_ID),
                    unique: false,
                    components: by_label_components,
                },
                DurableIndexShape {
                    id: LedgerIdBytes::from_bytes(BY_VALUE_INDEX_ID),
                    unique: true,
                    components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                        VALUE_FIELD_ID,
                    ))],
                },
            ],
        },
    });
    draft
}

/// The well-formed `byLabel` projection: the sparse `label` field then the identity
/// key, the complete-suffix shape a nonunique index requires.
fn by_label_projection() -> Vec<DurableIndexComponent> {
    vec![
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes(LABEL_FIELD_ID)),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
    ]
}

#[test]
fn a_well_formed_indexed_graph_verifies() {
    assert_eq!(
        code_of(&indexed_draft(by_label_projection()).encode().unwrap().bytes),
        "VERIFIED",
    );
}

#[test]
fn rehashed_mutated_index_id_breaks_the_contract_id() {
    // A managed index contributes its `Index` id to the durable graph the verifier
    // recomputes the contract over. Flipping that id and rehashing the outer digest
    // leaves the carried contract id stale, so the recomputation rejects — the
    // contract binds index identity, not only roots and fields.
    let mut bytes = indexed_draft(by_label_projection()).encode().unwrap().bytes;
    flip_ledger_id(&mut bytes, BY_VALUE_INDEX_ID);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn an_index_component_naming_no_leaf_of_its_root_rejects() {
    // A forged projection component pointing at a leaf the root does not carry is
    // refused at decode: the verifier re-resolves every component against the root's
    // own fields and keys.
    let forged = vec![
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes([0x99; 16])),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
    ];
    assert_eq!(
        code_of(&indexed_draft(forged).encode().unwrap().bytes),
        "image.table",
    );
}

/// A `Counter` root whose `owner` field is a widened dense struct, with a unique index
/// forging a component over that widened field. A widened field is executable (framed
/// inline in its cell) but never index-eligible — an index component must project an
/// orderable durable-key scalar, which a composite is not — so the verifier refuses the
/// component independently of the compiler. This is the index-eligibility decouple: the
/// same field keeps the root flat-executable yet stays out of every index.
fn widened_field_indexed_draft() -> ImageDraft {
    const OWNER_FIELD_ID: [u8; 16] = [0x1e; 16];
    let mut draft = ImageDraft::new();
    let name_ty = draft.intern_string("Name");
    let first = draft.intern_string("first");
    let last = draft.intern_string("last");
    let name_record = draft.add_record_type(RecordTypeDef {
        name: name_ty,
        fields: vec![
            FieldDef {
                name: first,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
            FieldDef {
                name: last,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
        ],
    });
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let owner = draft.intern_string("owner");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: owner,
                ty: ImageType::Record {
                    idx: name_record.index(),
                    optional: false,
                },
                required: true,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(OWNER_FIELD_ID),
                    required: true,
                    value: DurableValueShape::Struct(vec![
                        DurableValueShape::Scalar(Scalar::Text),
                        DurableValueShape::Scalar(Scalar::Text),
                    ]),
                },
            ],
            indexes: vec![DurableIndexShape {
                id: LedgerIdBytes::from_bytes(BY_VALUE_INDEX_ID),
                unique: true,
                components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                    OWNER_FIELD_ID,
                ))],
            }],
        },
    });
    draft
}

#[test]
fn an_index_component_over_a_widened_field_rejects() {
    // The widened `owner` field is admitted (its root is flat-executable), but naming it
    // as an index component is refused at decode — index eligibility is decoupled from
    // field executability, so a widened field is never an index leaf.
    assert_eq!(
        code_of(&widened_field_indexed_draft().encode().unwrap().bytes),
        "image.table",
    );
}

#[test]
fn a_site_that_claims_to_traverse_a_unique_index_rejects() {
    // The unique index `byValue` admits only a complete-key exact lookup. A forged
    // site with a progressive-prefix scan target over it — an attempt to traverse a
    // unique index and observe siblings — is refused when the site's read kind is
    // checked against the index's unique flag.
    let mut draft = indexed_draft(by_label_projection());
    draft.add_site(SiteDef::index_scan(index_path(BY_VALUE_INDEX_ID)));
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

#[test]
fn a_site_that_exact_looks_up_a_nonunique_index_rejects() {
    // Symmetrically, the nonunique `byLabel` admits only a progressive-prefix scan; a
    // forged complete-key lookup site over it is refused.
    let mut draft = indexed_draft(by_label_projection());
    draft.add_site(SiteDef::index_lookup(index_path(BY_LABEL_INDEX_ID)));
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

/// Flip the first occurrence of a 16-byte ledger id in `bytes`. The distinct test
/// ids never collide with a string or other field, so this reliably mutates the
/// targeted node.
fn flip_ledger_id(bytes: &mut [u8], id: [u8; 16]) {
    let at = bytes
        .windows(16)
        .position(|window| window == id)
        .expect("the ledger id appears in the image");
    bytes[at] ^= 0xFF;
}

#[test]
fn rehashed_mutated_group_id_breaks_the_contract_id() {
    // A static `group` namespace contributes its `Group` ledger id to the durable
    // member tree the verifier recomputes the contract over. Flipping that id (and
    // rehashing the outer digest) leaves the carried contract id stale, so the
    // recomputation rejects — the contract binds group structure, not only roots.
    let mut bytes = group_branch_draft(false).encode().unwrap().bytes;
    flip_ledger_id(&mut bytes, [0x20; 16]);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_mutated_branch_placement_id_breaks_the_contract_id() {
    // A keyed `branch` is a distinct placement; its id is part of the member tree
    // the contract binds. Flipping it and rehashing the digest leaves the carried
    // contract id stale, so the recomputation rejects.
    let mut bytes = group_branch_draft(false).encode().unwrap().bytes;
    flip_ledger_id(&mut bytes, [0x30; 16]);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn executable_site_over_a_group_bearing_root_rejects() {
    // A keyed root whose resource declares a static `group` is not yet executable — the
    // group's fields are part of the containing entry's materialized value, a model the
    // flat durable record does not yet carry — so every site over it parks. A forged image
    // that adds an operation site (here over the root's own `title` field) and an opcode is
    // refused independently during per-function typing (`image.function`), regardless of
    // the compiler's own boundary. (A keyed `branch` alone does not park a root; the group
    // does.)
    assert_eq!(
        code_of(&group_branch_draft(true).encode().unwrap().bytes),
        "image.function"
    );
}

/// The semantic path of the group/branch graph's `details.pages` group field:
/// application -> root placement -> group (`details`) -> field (`pages`). A group field's
/// path carries a `Group` step, distinct from a branch placement step.
fn group_field_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes([0x0a; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x0b; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Group,
            LedgerIdBytes::from_bytes([0x20; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x21; 16]),
        ),
    ])
}

#[test]
fn a_group_scoped_field_site_seals_parked() {
    // A site over a group-scoped field (`details.pages`) resolves against the
    // reconstructed node set and seals *parked* — its identity is complete, its execution
    // deferred until the group materialized-value model lands. No opcode references it, so
    // the image verifies.
    let mut draft = group_branch_draft(false);
    draft.add_site(SiteDef::field_leaf(group_field_path()));
    assert!(
        verify(&draft.encode().unwrap().bytes).is_ok(),
        "a group-scoped field site seals parked",
    );
}

#[test]
fn an_opcode_over_a_parked_group_field_site_rejects() {
    // A durable opcode that references a parked group-scoped field site is refused during
    // per-function typing (`image.function`), independently of the compiler's boundary — no
    // group operation reaches the runtime while groups park.
    let mut draft = group_branch_draft(false);
    let site = draft.add_site(SiteDef::field_leaf(group_field_path()));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("read");
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurReadField(site.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::opt_scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "read"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// The semantic path of the group/branch graph's `notes.text` branch field:
/// application -> root placement -> branch placement -> field (four steps), the
/// deepest concrete address in that graph.
fn branch_field_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes([0x0a; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x0b; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x30; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x32; 16]),
        ),
    ])
}

#[test]
fn a_deep_nested_branch_field_site_seals() {
    // A whole-graph site over the deepest concrete address — a keyed branch's
    // field — resolves against the reconstructed node set and seals (parked),
    // rather than being refused as not-yet-emitted. No opcode references it, so
    // the image verifies: its identity is complete, execution deferred.
    let mut draft = group_branch_draft(false);
    draft.add_site(SiteDef::field_leaf(branch_field_path()));
    assert!(
        verify(&draft.encode().unwrap().bytes).is_ok(),
        "a nested branch-field site seals"
    );
}

#[test]
fn a_branch_record_disagreeing_with_its_member_fields_rejects() {
    // A branch's materialized record is surface (not identity), so the verifier ties
    // it to the branch's own field members — order, value shape, and required flag —
    // exactly as a root's record ties to its member tree. Here the branch record
    // marks `text` sparse while the branch member marks it required; the image
    // encodes (identity is unchanged), but the independent record/member cross-check
    // refuses it at the table phase.
    let bytes = group_branch_draft_with_branch_record(false, false)
        .encode()
        .unwrap()
        .bytes;
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn an_opcode_over_a_parked_branch_field_site_rejects() {
    // The seal-but-park split: a nested branch-field site seals, but a durable opcode
    // that references it is refused during per-function typing (`image.function`) — a
    // parked site is not executable, independently of the compiler's own boundary.
    let mut draft = group_branch_draft(false);
    let site = draft.add_site(SiteDef::field_leaf(branch_field_path()));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("read");
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurReadField(site.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Text)],
        ret: ImageType::opt_scalar(Scalar::Text),
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "read"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// Build a flat-executable `Book { title:string required }` root at `^books(id:int)`
/// whose only extra is one single-level single-column-keyed scalar-field branch,
/// `notes(noteId:string)` holding `text:string required` — the executable branch
/// shape. No group, so the root is flat-executable and its branch whole-payload
/// site seals executable. Returns the draft and the branch's materialized record type
/// index (the whole branch-entry read's result type).
fn flat_branch_draft() -> (ImageDraft, u16) {
    let mut draft = ImageDraft::new();
    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Book.notes");
    let notes_text = draft.intern_string("text");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let root = draft.intern_string("books");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes([0x30; 16]),
                    name: notes,
                    record: notes_record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x31; 16]),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x32; 16]),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Text),
                    }],
                },
            ],
        },
    });
    (draft, notes_record.index())
}

/// The whole-payload site path of the flat-branch graph's `notes` branch entry:
/// application -> root placement -> branch placement (three steps).
fn branch_entry_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes([0x0a; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x0b; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x30; 16]),
        ),
    ])
}

#[test]
fn a_branch_whole_entry_read_over_a_flat_root_seals_and_type_checks() {
    // A single-level branch whole-payload site on a flat-executable root now seals
    // executable, and a read over it type-checks the two-element key-path
    // `[root_key, branch_key]` (int then string) and yields the branch's own record.
    let (mut draft, branch_record) = flat_branch_draft();
    let site = draft.add_site(SiteDef::whole_payload(branch_entry_path()));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("note");
    let code = vec![
        Instr::LocalGet(0), // id: the root key
        Instr::LocalGet(1), // noteId: the branch key, on top of the stack
        Instr::DurReadEntry(site.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

#[test]
fn a_branch_entry_op_missing_its_root_key_rejects() {
    // A branch entry op addresses `[root_key, branch_key]`. Pushing only the branch key
    // leaves the second (root) key pop with an empty stack — a key-arity forgery the
    // verifier refuses during per-function typing.
    let (mut draft, branch_record) = flat_branch_draft();
    let site = draft.add_site(SiteDef::whole_payload(branch_entry_path()));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("note");
    let code = vec![
        Instr::LocalGet(1), // only the branch key; the root key is missing
        Instr::DurReadEntry(site.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

#[test]
fn a_branch_entry_op_with_the_wrong_branch_key_type_rejects() {
    // The branch key column is `string`; pushing an `int` where the branch key belongs
    // is a type mismatch the two-element key-path check refuses.
    let (mut draft, branch_record) = flat_branch_draft();
    let site = draft.add_site(SiteDef::whole_payload(branch_entry_path()));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("note");
    let code = vec![
        Instr::LocalGet(0), // id: the root key (int)
        Instr::LocalGet(1), // an int where the branch key (string) belongs
        Instr::DurReadEntry(site.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Record {
            idx: branch_record,
            optional: true,
        },
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "note"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// A site path of exactly `n` steps: `Application`, `Placement`, then `n - 2` field
/// steps carrying the distinctive id `0x91` so the first step can be located in the
/// encoded bytes. The intermediate kinds are irrelevant to the length bound.
fn n_step_field_path(n: usize) -> SemanticPath {
    let mut steps = vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
    ];
    while steps.len() < n {
        steps.push(SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x91; 16]),
        ));
    }
    SemanticPath::from_steps(steps)
}

#[test]
fn a_site_path_at_the_maximum_depth_is_admitted_by_the_bound() {
    // A site path of exactly MAX_SITE_PATH_STEPS is admitted by the length gate (the
    // encoder builds it and the verifier's length check passes); it then fails only at
    // node resolution — `image.table` "names no graph node", not "too deep". This pins
    // the bound as inclusive at the maximum concrete-address depth.
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    draft.add_site(SiteDef::field_leaf(n_step_field_path(
        marrow_image::bounds::MAX_SITE_PATH_STEPS,
    )));
    let bytes = finish_two_key(
        draft,
        vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return],
    );
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn a_site_path_past_the_maximum_depth_is_refused_by_the_encoder() {
    // One step deeper than the bound: the encoder refuses to build the image, so an
    // over-deep site path can never be produced through the production path.
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    draft.add_site(SiteDef::field_leaf(n_step_field_path(
        marrow_image::bounds::MAX_SITE_PATH_STEPS + 1,
    )));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let code = vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    assert!(
        matches!(
            draft.encode(),
            Err(marrow_image::ImageBuildError::SitePathTooDeep)
        ),
        "the encoder refuses a site path past the maximum depth"
    );
}

#[test]
fn a_forged_over_deep_site_path_is_refused_by_the_verifier() {
    // Defense in depth for the branch the encoder guards: take the at-bound image,
    // bump only the forged site's step-count byte to one past the bound, and rehash.
    // The verifier's own length check trips before it decodes any step —
    // `image.table` "durable site path too deep" — so a forged image cannot smuggle an
    // unbounded path past the container.
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    draft.add_site(SiteDef::field_leaf(n_step_field_path(
        marrow_image::bounds::MAX_SITE_PATH_STEPS,
    )));
    let mut bytes = finish_two_key(
        draft,
        vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return],
    );
    // The forged site is the only one whose first step carries id 0x91; its step-count
    // byte immediately precedes that step's kind byte (`Application` = ledger kind 0).
    let first_step: [u8; 17] = {
        let mut pattern = [0u8; 17];
        pattern[0] = SemanticStepKind::Application.ledger_kind();
        pattern[1..].copy_from_slice(&APPLICATION_ID);
        pattern
    };
    // Find the forged path by its distinctive 0x91 field id, then walk back to the
    // Application step that opens the same path.
    let field_at = bytes
        .windows(16)
        .position(|w| w == [0x91; 16])
        .expect("the forged deep path is present");
    let step_kind_at = bytes[..field_at]
        .windows(17)
        .rposition(|w| w == first_step)
        .expect("the forged path opens with an Application step");
    assert_eq!(
        bytes[step_kind_at - 1],
        marrow_image::bounds::MAX_SITE_PATH_STEPS as u8,
        "the byte before the path is its step count",
    );
    bytes[step_kind_at - 1] = (marrow_image::bounds::MAX_SITE_PATH_STEPS + 1) as u8;
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("the forged over-deep path must reject");
    assert_eq!(rejection.code(), "image.table");
    assert_eq!(
        rejection.detail(),
        "durable site path too deep",
        "the length gate trips before any step is decoded",
    );
}

/// The tracer durable schema plus a verifying `put` export, with `extra` appended to
/// the site table. The image carries an encoder-computed digest, so any rejection is
/// the site table's own resolution, not a stale digest.
fn durable_with_extra_site(extra: SiteDef) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let (_entry, value_site, _label) = durable_schema(&mut draft);
    draft.add_site(extra);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().unwrap().bytes
}

#[test]
fn a_site_path_naming_no_graph_node_rejects() {
    // A field-leaf site whose path carries a ledger id absent from the durable graph
    // — the shape a mutated site-path id takes — resolves against no reconstructed
    // node and is refused at the durable table.
    let bytes = durable_with_extra_site(SiteDef::field_leaf(field_path([0xbb; 16])));
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn a_field_path_with_a_whole_payload_target_rejects() {
    // A whole-payload target over a field path: the path resolves to a field node,
    // but the target claims a keyed placement. A field site cannot acquire a
    // whole-payload target — the verifier rejects the kind disagreement at decode.
    let bytes = durable_with_extra_site(SiteDef::whole_payload(field_path(VALUE_FIELD_ID)));
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn a_root_path_with_a_field_leaf_target_rejects() {
    // The mirror hostile: a field-leaf target over the root's own path. The path
    // resolves to the placement node, but the target claims a stored field — a
    // retarget/rebind whose kind disagrees with the resolved node.
    let bytes = durable_with_extra_site(SiteDef::field_leaf(root_path()));
    assert_eq!(code_of(&bytes), "image.table");
}

/// Flip the *last* occurrence of a 16-byte ledger id — the copy carried in the site
/// table, which follows the member tree — leaving the graph's own id intact.
fn flip_last_ledger_id(bytes: &mut [u8], id: [u8; 16]) {
    let at = bytes
        .windows(16)
        .rposition(|window| window == id)
        .expect("the ledger id appears in the image");
    bytes[at] ^= 0xFF;
}

#[test]
fn rehashed_mutated_site_path_id_rejects_at_table() {
    // Flip only the site table's copy of the value field's ledger id (and rehash the
    // outer digest). The graph's member tree is untouched, so the contract id still
    // matches; but the site path now names an id absent from the graph and resolves
    // against no node. This is the site-path-mutation gate, distinct from the
    // contract-id gate a member-tree flip trips.
    let mut bytes = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(1),
        Instr::TxnCommit,
        Instr::Return,
    ])
    .encode()
    .unwrap()
    .bytes;
    flip_last_ledger_id(&mut bytes, [0x0e; 16]);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn flow_mutation_outside_transaction_rejects() {
    let value_site = 1;
    let draft = put_export(vec![
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

#[test]
fn flow_return_without_commit_rejects() {
    let value_site = 1;
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

#[test]
fn flow_double_begin_rejects() {
    let value_site = 1;
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

/// A durable operation after the commit is refused: the commit consumes the
/// session's engine transaction, so a read placed after it would reach a dead
/// transaction at runtime. Here the tape writes, commits, then reads the value
/// field — the flow lattice rejects the post-commit read.
#[test]
fn flow_durable_read_after_commit_rejects() {
    let value_site = 1;
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::LocalGet(0),
        Instr::DurReadField(value_site),
        Instr::Pop,
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

/// Add a single mutating export with two `string` key params (slots 0 and 1) over
/// the tracer schema in `draft`, whose body is `code`, and encode it. Used by the
/// presence-lattice hostiles, where the guard proves one slot and the strict set
/// names a slot. The caller interns any consts in the same draft first.
fn finish_two_key(mut draft: ImageDraft, code: Vec<Instr>) -> Vec<u8> {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

/// The well-formed shape: `if exists(p)` (LocalGet(S); DurExists(entry);
/// JumpIfFalse) dominates the strict set on its present edge, so the present-entry
/// sparse set verifies. The positive control the presence-lattice hostiles perturb.
#[test]
fn a_guarded_strict_sparse_set_verifies() {
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    let text = draft.intern_text("x");
    // JumpIfFalse targets the TxnCommit at instruction index 7 (the guard's absent
    // edge); the encoder maps the index to a byte offset.
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(0),
            Instr::JumpIfFalse(7),
            Instr::ConstLoad(text.index()),
            Instr::SomeWrap,
            Instr::DurSetSparsePresent {
                site: 2,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

/// A strict present-entry sparse set with no dominating presence fact on its key
/// slot is refused at the flow phase, independently of the compiler.
#[test]
fn a_strict_sparse_set_without_a_presence_fact_rejects() {
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    let text = draft.intern_text("x");
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::ConstLoad(text.index()),
            Instr::SomeWrap,
            Instr::DurSetSparsePresent {
                site: 2,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

/// The presence fact is proven for the guarded slot only: a strict set that names a
/// different, unproven key slot is refused even though that slot is initialized and
/// key-typed. This is the mutated-place-slot-index gate.
#[test]
fn a_strict_sparse_set_naming_an_unproven_slot_rejects() {
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    let text = draft.intern_text("x");
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(0),
            Instr::JumpIfFalse(7),
            Instr::ConstLoad(text.index()),
            Instr::SomeWrap,
            // Slot 0 is proven present by the guard; naming slot 1 is unproven.
            Instr::DurSetSparsePresent {
                site: 2,
                key_slots: vec![1],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

/// A presence fact established before a loop does not survive the loop header when the
/// body erases the entry through the same key slot. The header is a merge of the
/// pre-loop edge (slot 0 proven present by the `exists` guard) and the back edge (slot
/// 0 erased in the body); the intersection-join kills the fact, so a strict set placed
/// after the loop is refused at the flow phase. This pins the backedge kill explicitly:
/// without intersection at the header the stale pre-loop fact would wrongly dominate.
#[test]
fn a_strict_sparse_set_after_a_loop_that_erases_the_entry_rejects() {
    let mut draft = ImageDraft::new();
    durable_schema(&mut draft);
    let text = draft.intern_text("x");
    // Instruction-index layout (targets are draft-form indices):
    //   0 TxnBegin
    //   1 LocalGet(0); 2 DurExists(0); 3 JumpIfFalse(15) — present edge proves slot 0.
    //   4 loop header (merge of the pre-loop edge and the back edge at 9).
    //   4 LocalGet(1); 5 DurExists(0); 6 JumpIfFalse(10) — loop-continue test on slot 1.
    //   7 LocalGet(0); 8 DurEraseEntry(0) — body erases the entry keyed by slot 0.
    //   9 Jump(4) — back edge; slot 0 is absent on this edge.
    //   10 strict set on slot 0 (rejected: killed by the header intersection).
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(0),
            Instr::JumpIfFalse(15),
            Instr::LocalGet(1),
            Instr::DurExists(0),
            Instr::JumpIfFalse(10),
            Instr::LocalGet(0),
            Instr::DurEraseEntry(0),
            Instr::Jump(4),
            Instr::ConstLoad(text.index()),
            Instr::SomeWrap,
            Instr::DurSetSparsePresent {
                site: 2,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

/// The tracer schema plus a string-keyed `notes(noteId:string)` branch of one required
/// `text` field. The branch key is `string` to match the root key type, so a strict
/// root-field set naming the branch key slot type-checks — isolating the presence
/// lattice as the sole gate. Returns (draft, label field-leaf site, branch entry site,
/// branch record index).
fn branch_presence_schema() -> (ImageDraft, u16, u16, u16) {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Counter.notes");
    let notes_text = draft.intern_string("text");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let mut members = counters_members();
    members.push(DurableMemberDef::Branch {
        placement: LedgerIdBytes::from_bytes([0x30; 16]),
        name: notes,
        record: notes_record,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x31; 16]),
        }],
        members: vec![DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x32; 16]),
            required: true,
            value: DurableValueShape::Scalar(Scalar::Text),
        }],
    });
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members,
        },
    });
    draft.add_site(SiteDef::whole_payload(root_path()));
    draft.add_site(SiteDef::field_leaf(field_path(VALUE_FIELD_ID)));
    let label_site = draft.add_site(SiteDef::field_leaf(field_path(LABEL_FIELD_ID)));
    let branch_entry = draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x30; 16]),
        ),
    ])));
    (
        draft,
        label_site.index(),
        branch_entry.index(),
        notes_record.index(),
    )
}

/// A branch whole-entry create does not establish root-entry presence: it leaves the
/// root descendant-only, so its marker is still absent. A forged image that creates a
/// branch entry keyed by slot 1 and then does a strict present-entry root-field set
/// naming slot 1 — relying on the branch create to dominate it — is refused at the flow
/// phase. Without the `is_entry_site` gate in the presence lattice the branch create
/// would wrongly mark slot 1 present and this image would verify. The branch key is
/// `string` (the root key type), so the strict set type-checks and the presence lattice
/// is the sole gate.
#[test]
fn a_branch_create_does_not_dominate_a_strict_root_field_set_rejects() {
    let (mut draft, label_site, branch_entry, notes_record) = branch_presence_schema();
    let text = draft.intern_text("t");
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("e");
    // Slots: 0 = root key (string param), 1 = branch key (string param), 2 = the branch
    // record local (so the create matches the `LocalGet(rec); LocalGet(key)` shape the
    // presence lattice keys on).
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(text.index()),
        Instr::RecordNew(notes_record),
        Instr::LocalSet(2),
        Instr::LocalGet(0), // root key
        Instr::LocalGet(1), // branch key
        Instr::LocalGet(2), // branch record
        Instr::DurCreateEntry(branch_entry),
        Instr::ConstLoad(text.index()),
        Instr::SomeWrap,
        // Claims slot 1's *root* entry is present, relying on the branch create above.
        Instr::DurSetSparsePresent {
            site: label_site,
            key_slots: vec![1],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Unit,
        local_count: 3,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

/// The tracer schema plus a string-keyed `notes(noteId:string)` branch of one *sparse*
/// `body:text` field, and a site on that branch field. The branch key is `string` (the
/// root key type) so a strict set naming the root key slot type-checks, and the field is
/// sparse so it clears the required-field gate — isolating the site-target check as the
/// sole remaining gate. Returns (draft, root whole-payload site, branch-field site).
fn branch_field_schema() -> (ImageDraft, u16, u16) {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Counter.notes");
    let notes_body = draft.intern_string("body");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_body,
            ty: ImageType::scalar(Scalar::Text),
            required: false,
        }],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let mut members = counters_members();
    members.push(DurableMemberDef::Branch {
        placement: LedgerIdBytes::from_bytes([0x30; 16]),
        name: notes,
        record: notes_record,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x31; 16]),
        }],
        members: vec![DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x32; 16]),
            required: false,
            value: DurableValueShape::Scalar(Scalar::Text),
        }],
    });
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members,
        },
    });
    let root_entry = draft.add_site(SiteDef::whole_payload(root_path()));
    let branch_field = draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes([0x30; 16]),
        ),
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x32; 16]),
        ),
    ])));
    (draft, root_entry.index(), branch_field.index())
}

/// A branch-field site's key-path is the two-element `[root_key, branch_key]`. The strict
/// present-entry sparse set carries one slot per key column, so a forged image that proves
/// the root entry present with an `exists` guard and then drives the strict set over a
/// branch-field site supplying only the *root* key (one slot) is a key-path arity mismatch
/// and must be refused at the function phase. Accepting it would let the kernel drop the
/// branch hop and mis-address the write. (The slice-A write-safety concern; the correct
/// two-slot branch strict set is admitted and exercised through the production path.)
#[test]
fn a_strict_sparse_set_over_a_branch_field_with_a_single_root_key_rejects() {
    let (mut draft, root_entry, branch_field) = branch_field_schema();
    let text = draft.intern_text("x");
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("e");
    // Slot 0 is the root key (string param). The guard `LocalGet(0); DurExists(root
    // whole payload); JumpIfFalse` proves slot 0's root entry present on its taken edge;
    // the strict set then names the branch-field site with only that one slot — a
    // one-element key-path over a two-element branch-field site.
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurExists(root_entry),
        Instr::JumpIfFalse(7),
        Instr::ConstLoad(text.index()),
        Instr::SomeWrap,
        Instr::DurSetSparsePresent {
            site: branch_field,
            key_slots: vec![0],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Text)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// A two-slot branch-field strict set with the correct `[root, branch]` key-path arity and
/// key types, but with no `exists`/`if const` guard dominating it, passes the arity and
/// type checks yet is refused at the flow phase: the key-path presence lattice holds no
/// fact for the branch entry `[0, 1]`. This isolates the presence requirement of the
/// generalized (branch) strict form from its arity/type gate.
#[test]
fn a_two_slot_branch_strict_set_without_a_presence_fact_rejects() {
    let (mut draft, _root_entry, branch_field) = branch_field_schema();
    let text = draft.intern_text("x");
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("e");
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(text.index()),
        Instr::SomeWrap,
        // Slots 0,1 are the [root, branch] key params — arity- and type-correct for the
        // branch-field site — but no guard proves the branch entry present.
        Instr::DurSetSparsePresent {
            site: branch_field,
            key_slots: vec![0, 1],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

#[test]
fn flow_transaction_owner_may_not_be_called_rejects() {
    // A helper owns a transaction (contains TxnBegin); an export that calls it is a
    // flow violation — helpers cannot own the transaction.
    let mut draft = ImageDraft::new();
    let (_entry, value_site, _label) = durable_schema(&mut draft);
    let src = draft.intern_string("src/main.mw");
    let key = draft.intern_text("x");
    let val = draft.intern_int(1);
    let helper_name = draft.intern_string("helper");
    let helper_code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(key.index()),
        Instr::ConstLoad(val.index()),
        Instr::DurSetRequired(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let helper = draft.add_function(FunctionDef {
        name: helper_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&helper_code),
        code: helper_code,
    });
    let main_name = draft.intern_string("main");
    let main_code = vec![Instr::Call(helper.index()), Instr::Return];
    let main = draft.add_function(FunctionDef {
        name: main_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&main_code),
        code: main_code,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

#[test]
fn set_sparse_on_a_required_field_rejects_at_function() {
    // Targeting the required `value` field with the sparse opcode is a phase-3
    // site/target error.
    let value_site = 1;
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::VacantLoad(ImageType::opt_scalar(Scalar::Int)),
        Instr::DurSetSparse(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

#[test]
fn create_on_a_field_site_rejects_at_function() {
    // `create` requires an entry site; a field-target site is a phase-3 error.
    let value_site = 1;
    let draft = put_export(vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurCreateEntry(value_site),
        Instr::TxnCommit,
        Instr::Return,
    ]);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

// --- TEST-ENTRY section and OP_ASSERT hostiles (P00b). ---

/// A well-formed image with one storeless test entry whose body asserts a
/// constant true, returning the draft and the test function's id. The
/// TEST-ENTRY hostiles derive from this.
fn test_entry_image() -> (ImageDraft, FuncId) {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let truth = draft.intern_bool(true);
    let code = vec![
        Instr::ConstLoad(truth.index()),
        Instr::Assert,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    (draft, func)
}

#[test]
fn assert_in_a_test_entry_verifies() {
    // The well-formed baseline the TEST-ENTRY hostiles derive from.
    let (draft, _) = test_entry_image();
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

#[test]
fn assert_outside_a_test_entry_rejects() {
    // The same asserting function, exported instead of test-entered: `assert`
    // is legal only inside a test entry.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let truth = draft.intern_bool(true);
    let code = vec![
        Instr::ConstLoad(truth.index()),
        Instr::Assert,
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
    draft.add_export(ExportId::of_local("", "f"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
}

#[test]
fn assert_on_a_non_bool_operand_rejects_at_function() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let seven = draft.intern_int(7);
    let code = vec![
        Instr::ConstLoad(seven.index()),
        Instr::Assert,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

#[test]
fn test_entry_that_is_also_an_export_rejects() {
    let (mut draft, func) = test_entry_image();
    draft.add_export(ExportId::of_local("", "holds"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
}

#[test]
fn test_entry_with_a_parameter_rejects() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let code = vec![Instr::LocalGet(0), Instr::Assert, Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: vec![ImageType::scalar(Scalar::Bool)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
}

#[test]
fn test_entry_with_a_non_unit_return_rejects() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let seven = draft.intern_int(7);
    let code = vec![Instr::ConstLoad(seven.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
}

#[test]
fn test_entry_may_carry_durable_demand() {
    // A test entry whose body probes durable data verifies: its demand is
    // recorded in the parallel test-entry demand table so an ephemeral
    // attachment can bound its authority. It is still never an export.
    let mut draft = ImageDraft::new();
    let (entry_site, _value, _label) = durable_schema(&mut draft);
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let key = draft.intern_text("x");
    let code = vec![
        Instr::ConstLoad(key.index()),
        Instr::DurExists(entry_site),
        Instr::Assert,
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    let image = verify(&draft.encode().unwrap().bytes).expect("durable test entry verifies");
    let entry = &image.test_entries()[0];
    assert_eq!(entry.name(), "holds");
    // A presence probe on the root: the test-image demand union is nonempty and
    // reads without writing.
    let union = image.test_demand_union();
    assert!(!union.is_empty());
    assert!(union.reads());
    assert!(!union.writes());
    // A test entry is never an export.
    assert!(image.exports().is_empty());
}

#[test]
fn test_entry_as_a_call_target_rejects() {
    // An exported function that calls the test entry: a test entry is an entry
    // point and may never be a call target.
    let (mut draft, _) = test_entry_image();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let code = vec![Instr::Call(0), Instr::Return];
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
}

/// The TEST-ENTRY section frame (id 8) of an encoded image.
fn test_entry_section(bytes: &[u8]) -> (usize, usize) {
    let (_, body, len) = *sections(bytes).iter().find(|(id, ..)| *id == 8).unwrap();
    (body, len)
}

/// A two-test image whose TEST-ENTRY section rows the byte-patch hostiles edit.
fn two_test_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let truth = draft.intern_bool(true);
    for title_text in ["alpha", "beta"] {
        let title = draft.intern_string(title_text);
        let code = vec![
            Instr::ConstLoad(truth.index()),
            Instr::Assert,
            Instr::Return,
        ];
        let func = draft.add_function(FunctionDef {
            name: title,
            source: src,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_test_entry(title, func);
    }
    draft.encode().expect("encode").bytes
}

#[test]
fn rehashed_test_entry_function_out_of_range_rejects_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    // Row layout: count(u16), then per row name(u16) func(u16).
    let func_field = body + 2 + 2;
    bytes[func_field] = 0xFF;
    bytes[func_field + 1] = 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_test_entry_name_out_of_range_rejects_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    let name_field = body + 2;
    bytes[name_field] = 0xFF;
    bytes[name_field + 1] = 0xFF;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_duplicate_test_entry_name_rejects_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    // Copy the first row's name onto the second row: names must strictly ascend.
    let first_name = [bytes[body + 2], bytes[body + 3]];
    bytes[body + 6..body + 8].copy_from_slice(&first_name);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_descending_test_entry_names_reject_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    // Swap the two 4-byte rows so their name indices descend.
    let row0: [u8; 4] = bytes[body + 2..body + 6].try_into().unwrap();
    let row1: [u8; 4] = bytes[body + 6..body + 10].try_into().unwrap();
    bytes[body + 2..body + 6].copy_from_slice(&row1);
    bytes[body + 6..body + 10].copy_from_slice(&row0);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_test_entry_count_past_body_rejects_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    // Claim three rows while the body carries two: the third row read runs short.
    bytes[body..body + 2].copy_from_slice(&3u16.to_be_bytes());
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_test_entry_count_short_of_body_rejects_at_table() {
    let mut bytes = two_test_image();
    let (body, _) = test_entry_section(&bytes);
    // Claim one row while the body carries two: the second row is trailing bytes.
    bytes[body..body + 2].copy_from_slice(&1u16.to_be_bytes());
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn rehashed_aliased_test_entry_function_rejects() {
    // Assert-free bodies isolate the aliasing rule: after the patch the orphaned
    // function carries no `Assert`, so only the two-names-one-function check can
    // reject (an assert-bearing body would trip the assert-outside-a-test-entry
    // rule with the same code and mask a revert of the aliasing check).
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    for title_text in ["alpha", "beta"] {
        let title = draft.intern_string(title_text);
        let code = vec![Instr::Return];
        let func = draft.add_function(FunctionDef {
            name: title,
            source: src,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        });
        draft.add_test_entry(title, func);
    }
    let mut bytes = draft.encode().expect("encode").bytes;
    assert!(marrow_verify::verify(&bytes).is_ok());
    let (body, _) = test_entry_section(&bytes);
    // Point the second row's function at the first row's function: two names may
    // not alias one test function.
    let first_func = [bytes[body + 4], bytes[body + 5]];
    bytes[body + 8..body + 10].copy_from_slice(&first_func);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.test_entry");
}

#[test]
fn transaction_marker_in_a_test_entry_rejects_at_flow() {
    // A TxnBegin inside a test entry: a transaction marker may only sit in a
    // mutating export entry, so the flow phase rejects it before the TestEntry
    // phase ever runs.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let title = draft.intern_string("holds");
    let code = vec![Instr::TxnBegin, Instr::TxnCommit, Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: title,
        source: src,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_test_entry(title, func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.flow");
}

/// A well-formed range guard over a bare int verifies: it peeks the int and
/// leaves the stack unchanged, so the guarded value still returns.
#[test]
fn range_guard_over_a_bare_int_verifies() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let seven = draft.intern_int(7);
    let code = vec![
        Instr::ConstLoad(seven.index()),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

/// A range guard with nothing on the stack rejects at per-function
/// verification: the guard peeks its operand, so an operand must exist.
#[test]
fn range_guard_on_an_empty_stack_rejects_at_function() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let seven = draft.intern_int(7);
    let code = vec![
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::ConstLoad(seven.index()),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// A range guard over a non-int (here a bool) rejects at per-function
/// verification: the guarded value must be a bare int.
#[test]
fn range_guard_on_a_non_int_rejects_at_function() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let flag = draft.intern_bool(true);
    let code = vec![
        Instr::ConstLoad(flag.index()),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Bool),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// A range guard whose interval is empty (`lo > hi`) rejects at decode: no
/// value satisfies it, so a compiler never emits one and an image carrying one
/// is hostile.
#[test]
fn range_guard_with_an_empty_interval_rejects_at_function() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let seven = draft.intern_int(7);
    let code = vec![
        Instr::ConstLoad(seven.index()),
        Instr::RangeGuard { lo: 5, hi: 4 },
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// A function body truncated mid-way through a range guard's 16-byte interval
/// immediate rejects at per-function verification (a short operand), not with
/// a panic or an out-of-bounds read. The truncation shortens the code length,
/// the section frame, and the payload consistently, then rehashes, so only the
/// operand-boundary invariant is violated.
#[test]
fn range_guard_with_a_truncated_operand_rejects_at_function() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let seven = draft.intern_int(7);
    let code = vec![
        Instr::ConstLoad(seven.index()),
        Instr::RangeGuard { lo: 0, hi: 150 },
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        // One span at instruction 0, so span offsets stay valid after the cut.
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 1,
            column: 1,
        }],
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), func);
    let mut bytes = draft.encode().unwrap().bytes;

    // Locate the FUNCTIONS section (id 0x05) and its one function's code-length
    // field: body = u16 count, u16 name, u16 source, u8 param_count, ret tag,
    // u16 local_count, u32 code_len, code bytes.
    let (offset, len) = sections(&bytes)
        .into_iter()
        .find_map(|(id, offset, len)| (id == 0x05).then_some((offset, len)))
        .expect("functions section");
    let code_len_at = offset + 2 + 2 + 2 + 1 + 1 + 2;
    let code_len =
        u32::from_be_bytes(bytes[code_len_at..code_len_at + 4].try_into().unwrap()) as usize;
    let code_end = code_len_at + 4 + code_len;
    assert_eq!(code_end, offset + len, "one function fills the section");

    // Cut the final 9 bytes: the Return opcode plus the interval's second i64,
    // leaving the RangeGuard opcode with a short immediate at end of code.
    let cut = 9;
    bytes.drain(code_end - cut..code_end);
    let new_code_len = (code_len - cut) as u32;
    bytes[code_len_at..code_len_at + 4].copy_from_slice(&new_code_len.to_be_bytes());
    let new_section_len = (len - cut) as u32;
    bytes[offset - 4..offset].copy_from_slice(&new_section_len.to_be_bytes());
    rehash(&mut bytes);

    assert_eq!(code_of(&bytes), "image.function");
}

// --- record-typed parameter and return references (V3b) ---

/// A one-int-field record type, a function that constructs and returns it, and a
/// function that takes it by value and reads a field all verify — the well-formed
/// baseline the record-ref hostiles derive from.
#[test]
fn record_param_and_return_refs_verify() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let field = draft.intern_string("x");
    let rec = draft.add_record_type(RecordTypeDef {
        name: field,
        fields: vec![FieldDef {
            name: field,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let zero = draft.intern_int(0);
    let make_name = draft.intern_string("make");
    let make_code = vec![
        Instr::ConstLoad(zero.index()),
        Instr::RecordNew(rec.index()),
        Instr::Return,
    ];
    draft.add_function(FunctionDef {
        name: make_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::Record {
            idx: rec.index(),
            optional: false,
        },
        local_count: 0,
        spans: spans(&make_code),
        code: make_code,
    });
    let take_name = draft.intern_string("take");
    let take_code = vec![Instr::LocalGet(0), Instr::FieldGet(0), Instr::Return];
    let take = draft.add_function(FunctionDef {
        name: take_name,
        source: src,
        params: vec![ImageType::Record {
            idx: rec.index(),
            optional: false,
        }],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&take_code),
        code: take_code,
    });
    draft.add_export(ExportId::of_local("", "take"), take);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "VERIFIED");
}

/// A record return type index past the type table rejects at the table phase.
#[test]
fn record_return_index_out_of_range_rejects() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let code = vec![Instr::Return];
    let f = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        // No record types exist, so index 5 is out of range.
        ret: ImageType::Record {
            idx: 5,
            optional: false,
        },
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), f);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

/// A record parameter type index past the type table rejects at the table phase.
#[test]
fn record_param_index_out_of_range_rejects() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let code = vec![Instr::Return];
    let f = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::Record {
            idx: 5,
            optional: false,
        }],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), f);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

/// An optional parameter type is outside the parameter subset and rejects at the
/// table phase.
#[test]
fn optional_parameter_type_rejects() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let code = vec![Instr::Return];
    let f = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::opt_scalar(Scalar::Int)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), f);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

// --- record field-type table hostiles (C02 V5: fields are bare scalar or enum). ---

/// Build a minimal image whose single record type's fields come from `fields`,
/// plus a trivial `fn f(): int` export. The record table decodes before the
/// function, so a malformed field type rejects at the table phase.
fn record_table_image(fields: impl FnOnce(&mut ImageDraft) -> Vec<FieldDef>) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let rname = draft.intern_string("R");
    let field_defs = fields(&mut draft);
    draft.add_record_type(RecordTypeDef {
        name: rname,
        fields: field_defs,
    });
    let zero = draft.intern_int(0);
    let fname = draft.intern_string("f");
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: fname,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn record_field_with_optional_type_rejects() {
    // A field type is bare; an optional flag on it rejects at the table phase.
    let bytes = record_table_image(|draft| {
        let name = draft.intern_string("x");
        vec![FieldDef {
            name,
            ty: ImageType::opt_scalar(Scalar::Int),
            required: true,
        }]
    });
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn record_field_with_out_of_range_enum_index_rejects() {
    // An enum-typed field naming an index past the (empty) enum table rejects.
    let bytes = record_table_image(|draft| {
        let name = draft.intern_string("x");
        vec![FieldDef {
            name,
            ty: ImageType::Enum {
                idx: 3,
                optional: false,
            },
            required: true,
        }]
    });
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn value_type_cycle_through_a_record_field_rejects() {
    // Record R (idx 0) has an enum field E (idx 0), whose one variant carries a
    // Record(0) payload: a value type that contains itself. The combined
    // record+enum acyclicity pass rejects it.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let rname = draft.intern_string("R");
    let ename = draft.intern_string("E");
    let vname = draft.intern_string("wrap");
    let fname = draft.intern_string("inner");
    draft.add_record_type(RecordTypeDef {
        name: rname,
        fields: vec![FieldDef {
            name: fname,
            ty: ImageType::Enum {
                idx: 0,
                optional: false,
            },
            required: true,
        }],
    });
    draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Record {
                idx: 0,
                optional: false,
            }],
        }],
    });
    let zero = draft.intern_int(0);
    let f = draft.intern_string("f");
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: f,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}

/// Add a trivial `fn f(): int` export to `draft`, encode, and return the rejection
/// code (or `""` for a clean image). Shared by the value-graph hostiles below, which
/// populate the record and enum tables before calling this.
fn value_graph_code(draft: &mut ImageDraft) -> String {
    let src = draft.intern_string("src/main.mw");
    let zero = draft.intern_int(0);
    let fname = draft.intern_string("f");
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: fname,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    code_of(&draft.encode().unwrap().bytes)
}

#[test]
fn record_field_with_out_of_range_record_index_rejects() {
    // A struct-typed field naming a record index past the RECORD-TYPES table rejects
    // before the acyclicity pass.
    let bytes = record_table_image(|draft| {
        let name = draft.intern_string("x");
        vec![FieldDef {
            name,
            ty: ImageType::Record {
                idx: 5,
                optional: false,
            },
            required: true,
        }]
    });
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn self_referential_record_field_rejects() {
    // Record 0 has a field of type Record(0): a value that directly contains itself.
    let bytes = record_table_image(|draft| {
        let name = draft.intern_string("me");
        vec![FieldDef {
            name,
            ty: ImageType::Record {
                idx: 0,
                optional: false,
            },
            required: true,
        }]
    });
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn value_type_cycle_through_two_records_rejects() {
    // Record 0 has a field of type Record(1) and Record 1 a field of type Record(0):
    // a struct-to-struct cycle the widened record-field edge now catches.
    let mut draft = ImageDraft::new();
    let a = draft.intern_string("A");
    let b = draft.intern_string("B");
    let fb = draft.intern_string("b");
    let fa = draft.intern_string("a");
    draft.add_record_type(RecordTypeDef {
        name: a,
        fields: vec![FieldDef {
            name: fb,
            ty: ImageType::Record {
                idx: 1,
                optional: false,
            },
            required: true,
        }],
    });
    draft.add_record_type(RecordTypeDef {
        name: b,
        fields: vec![FieldDef {
            name: fa,
            ty: ImageType::Record {
                idx: 0,
                optional: false,
            },
            required: true,
        }],
    });
    assert_eq!(value_graph_code(&mut draft), "image.table");
}

#[test]
fn self_referential_enum_payload_rejects() {
    // Enum 0's one variant carries a payload of type Enum(0): a value that contains
    // itself with no record on the cycle.
    let mut draft = ImageDraft::new();
    let ename = draft.intern_string("E");
    let vname = draft.intern_string("wrap");
    draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Enum {
                idx: 0,
                optional: false,
            }],
        }],
    });
    assert_eq!(value_graph_code(&mut draft), "image.table");
}

#[test]
fn value_type_cycle_through_mixed_records_and_enums_rejects() {
    // A three-hop cycle Record0 -> Enum0 -> Record1 -> Record0 mixing record fields
    // and an enum payload leaf.
    let mut draft = ImageDraft::new();
    let r0 = draft.intern_string("R0");
    let r1 = draft.intern_string("R1");
    let ename = draft.intern_string("E");
    let vname = draft.intern_string("wrap");
    let f_e = draft.intern_string("e");
    let f_back = draft.intern_string("back");
    draft.add_record_type(RecordTypeDef {
        name: r0,
        fields: vec![FieldDef {
            name: f_e,
            ty: ImageType::Enum {
                idx: 0,
                optional: false,
            },
            required: true,
        }],
    });
    draft.add_record_type(RecordTypeDef {
        name: r1,
        fields: vec![FieldDef {
            name: f_back,
            ty: ImageType::Record {
                idx: 0,
                optional: false,
            },
            required: true,
        }],
    });
    draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: vec![ImageType::Record {
                idx: 1,
                optional: false,
            }],
        }],
    });
    assert_eq!(value_graph_code(&mut draft), "image.table");
}

#[test]
fn deep_acyclic_record_chain_verifies() {
    // A long but acyclic chain Record0 -> Record1 -> ... -> Record(N-1) verifies:
    // depth is not a restriction, only cycles are.
    let mut draft = ImageDraft::new();
    const N: u16 = 24;
    for i in 0..N {
        let name = draft.intern_string(&format!("R{i}"));
        let fields = if i + 1 < N {
            let fname = draft.intern_string("next");
            vec![FieldDef {
                name: fname,
                ty: ImageType::Record {
                    idx: i + 1,
                    optional: false,
                },
                required: true,
            }]
        } else {
            let fname = draft.intern_string("v");
            vec![FieldDef {
                name: fname,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }]
        };
        draft.add_record_type(RecordTypeDef { name, fields });
    }
    assert_eq!(value_graph_code(&mut draft), "VERIFIED");
}

/// A two-variant payloadless enum's durable value shape, with a sum id and one
/// member id per variant.
fn access_enum_value(members: Vec<DurableEnumMemberShape>) -> DurableValueShape {
    DurableValueShape::Enum {
        sum: LedgerIdBytes::from_bytes([0x50; 16]),
        members,
    }
}

/// Two payloadless members with distinct ids, matching a `reader`/`writer` enum.
fn access_members() -> Vec<DurableEnumMemberShape> {
    vec![
        DurableEnumMemberShape {
            id: LedgerIdBytes::from_bytes([0x51; 16]),
            payload: Vec::new(),
        },
        DurableEnumMemberShape {
            id: LedgerIdBytes::from_bytes([0x52; 16]),
            payload: Vec::new(),
        },
    ]
}

/// A valid widened durable image: `Widget { id:int required, kind:Access required }`
/// stored at `^widgets(id:int)`, where `Access` is a two-variant payloadless enum.
/// The `kind` field's durable value shape is a closed enum carrying a sum id and one
/// member id per variant, so the member tree matches the materialized record's
/// widened value shape. These fixtures exercise the widened durable member shape at
/// seal, so the root carries no operation sites and a pure function completes the
/// storeless image; the widened field's executability is covered elsewhere.
fn widened_draft(kind_value: DurableValueShape) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let access = draft.intern_string("Access");
    let reader = draft.intern_string("reader");
    let writer = draft.intern_string("writer");
    draft.add_enum_type(EnumTypeDef {
        name: access,
        variants: vec![
            VariantDef {
                name: reader,
                category: false,
                payload: Vec::new(),
            },
            VariantDef {
                name: writer,
                category: false,
                payload: Vec::new(),
            },
        ],
    });
    let widget = draft.intern_string("Widget");
    let idn = draft.intern_string("id");
    let kindn = draft.intern_string("kind");
    let rec = draft.add_record_type(RecordTypeDef {
        name: widget,
        fields: vec![
            FieldDef {
                name: idn,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: kindn,
                ty: ImageType::Enum {
                    idx: 0,
                    optional: false,
                },
                required: true,
            },
        ],
    });
    let root = draft.intern_string("widgets");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record: rec,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0f; 16]),
                    required: true,
                    value: kind_value,
                },
            ],
        },
    });
    let zero = draft.intern_int(0);
    let f = draft.intern_string("f");
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: f,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft
}

#[test]
fn a_widened_enum_field_image_verifies() {
    // A durable resource with a closed-enum field is now identity-complete and
    // verifies when its member tree's value shape matches the record's enum field.
    assert_eq!(
        code_of(
            &widened_draft(access_enum_value(access_members()))
                .encode()
                .unwrap()
                .bytes
        ),
        "VERIFIED"
    );
}

#[test]
fn rehashed_mutated_enum_member_id_breaks_the_contract_id() {
    // An enum member id (kind 6) is part of the durable member tree the verifier
    // recomputes the contract over. Flipping it and rehashing the outer digest leaves
    // the carried contract id stale, so the recomputation rejects — the contract
    // binds each member's identity, so append-only evolution has stable codes.
    let mut bytes = widened_draft(access_enum_value(access_members()))
        .encode()
        .unwrap()
        .bytes;
    flip_ledger_id(&mut bytes, [0x51; 16]);
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

#[test]
fn forged_duplicate_enum_member_id_rejects() {
    // Entropy-minted ids are pairwise distinct; two members claiming one id forge a
    // duplicate identity in the durable table and reject before the contract
    // recomputation, so a hostile image cannot alias two members to one code.
    let dup = vec![
        DurableEnumMemberShape {
            id: LedgerIdBytes::from_bytes([0x51; 16]),
            payload: Vec::new(),
        },
        DurableEnumMemberShape {
            id: LedgerIdBytes::from_bytes([0x51; 16]),
            payload: Vec::new(),
        },
    ];
    assert_eq!(
        code_of(
            &widened_draft(access_enum_value(dup))
                .encode()
                .unwrap()
                .bytes
        ),
        "image.table"
    );
}

#[test]
fn enum_value_shape_that_mismatches_the_record_rejects() {
    // The member tree's value shape must match the materialized record's enum field.
    // A value shape with three members when the enum table has two variants cannot be
    // reconciled, so the cross-check rejects — a hostile image cannot claim one
    // durable identity while its executable record carries a different value shape.
    let mut extra = access_members();
    extra.push(DurableEnumMemberShape {
        id: LedgerIdBytes::from_bytes([0x53; 16]),
        payload: Vec::new(),
    });
    assert_eq!(
        code_of(
            &widened_draft(access_enum_value(extra))
                .encode()
                .unwrap()
                .bytes
        ),
        "image.table"
    );
}

#[test]
fn out_of_domain_durable_value_tag_rejects() {
    // The durable value shape is self-describing (scalar 0, struct 1, enum 2).
    // Mutating the `kind` field's value tag to an unknown value (and rehashing the
    // digest) is an out-of-domain value shape the decoder refuses.
    let mut bytes = widened_draft(access_enum_value(access_members()))
        .encode()
        .unwrap()
        .bytes;
    // Find the `kind` field (member id 0x0f) followed by its required flag (0x01) and
    // its value tag (0x02, enum), and corrupt the tag.
    let mut needle = vec![0x0f_u8; 16];
    needle.extend_from_slice(&[0x01, 0x02]);
    let at = bytes
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .expect("the kind field value tag appears in the image");
    bytes[at + needle.len() - 1] = 0x7f;
    rehash(&mut bytes);
    assert_eq!(code_of(&bytes), "image.table");
}

// --- Nested-branch admission hostiles (E03w slice B) ---
//
// A nested single-column scalar-field branch is executable: `^books(id).notes(nid).tags(tid)`
// addresses a durable node two levels below the root. The verifier resolves a branch site's
// path level by level through the reconstructed member tree; a path that routes a branch
// under a field, or names a branch that does not exist at its level, resolves to no branch
// and seals *parked*, so a durable opcode over it is refused rather than mis-addressed.

/// Build a flat-executable `Book { title }` root at `^books(id:int)` whose `notes` branch
/// (`noteId:string`, `text:string required`) itself holds a nested `tags` branch
/// (`tagId:int`, `weight:int required`) — the executable nested-branch shape. The verifier
/// seals the whole recursive branch tree, so a valid deep site seals executable.
fn nested_branch_draft() -> ImageDraft {
    let mut draft = ImageDraft::new();
    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Book.notes");
    let notes_text = draft.intern_string("text");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let tags = draft.intern_string("tags");
    let tags_qualified = draft.intern_string("Book.notes.tags");
    let tags_weight = draft.intern_string("weight");
    let tags_record = draft.add_record_type(RecordTypeDef {
        name: tags_qualified,
        fields: vec![FieldDef {
            name: tags_weight,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let root = draft.intern_string("books");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes([0x30; 16]),
                    name: notes,
                    record: notes_record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x31; 16]),
                    }],
                    members: vec![
                        DurableMemberDef::Field {
                            id: LedgerIdBytes::from_bytes([0x32; 16]),
                            required: true,
                            value: DurableValueShape::Scalar(Scalar::Text),
                        },
                        DurableMemberDef::Branch {
                            placement: LedgerIdBytes::from_bytes([0x40; 16]),
                            name: tags,
                            record: tags_record,
                            keys: vec![KeyColumn {
                                scalar: Scalar::Int,
                                id: LedgerIdBytes::from_bytes([0x41; 16]),
                            }],
                            members: vec![DurableMemberDef::Field {
                                id: LedgerIdBytes::from_bytes([0x42; 16]),
                                required: true,
                                value: DurableValueShape::Scalar(Scalar::Int),
                            }],
                        },
                    ],
                },
            ],
        },
    });
    draft
}

/// A step in the `books` graph.
fn step(kind: SemanticStepKind, id: u8) -> SemanticStep {
    SemanticStep::new(kind, LedgerIdBytes::from_bytes([id; 16]))
}

/// The whole-payload site path of the nested `tags` branch entry from a chain of steps
/// below the root: `application -> root placement -> <chain>`.
fn nested_path(chain: &[SemanticStep]) -> SemanticPath {
    let mut steps = vec![
        step(SemanticStepKind::Application, 0x0a),
        step(SemanticStepKind::Placement, 0x0b),
    ];
    steps.extend_from_slice(chain);
    SemanticPath::from_steps(steps)
}

/// Add a read-only export that runs `DurExists` over the whole-payload `site` at the given
/// key arity (root-first `int, string, int` for the tag entry), and encode. The opcode is
/// the observation that separates an executable deep site from a parked one.
fn exists_over_tag_entry(mut draft: ImageDraft, site: u16) -> Vec<u8> {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("has");
    let code = vec![
        Instr::LocalGet(0), // root key: int
        Instr::LocalGet(1), // note key: string
        Instr::LocalGet(2), // tag key: int
        Instr::DurExists(site),
        Instr::Return,
    ];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::scalar(Scalar::Bool),
        local_count: 3,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "has"), func);
    draft.encode().unwrap().bytes
}

#[test]
fn a_valid_deep_nested_branch_entry_site_seals_executable_and_its_opcode_verifies() {
    // The positive control: a whole-payload site over the concrete `notes -> tags` chain
    // resolves to the nested branch, seals executable, and an `exists` opcode over its
    // three-column key-path type-checks and verifies.
    let mut draft = nested_branch_draft();
    let site = draft
        .add_site(SiteDef::whole_payload(nested_path(&[
            step(SemanticStepKind::Placement, 0x30),
            step(SemanticStepKind::Placement, 0x40),
        ])))
        .index();
    assert_eq!(code_of(&exists_over_tag_entry(draft, site)), "VERIFIED");
}

#[test]
fn a_branch_path_routed_through_a_field_rejects_at_the_table_phase() {
    // A forged path that routes the `tags` branch under the `text` *field* of `notes`
    // (a field has no branch child) names no reconstructed durable node, so the site is
    // refused when the verifier resolves it against its own node set at the table phase —
    // before any function, and independently of whether an opcode references it.
    let mut draft = nested_branch_draft();
    let site = draft
        .add_site(SiteDef::whole_payload(nested_path(&[
            step(SemanticStepKind::Placement, 0x30),
            step(SemanticStepKind::Field, 0x32),
            step(SemanticStepKind::Placement, 0x40),
        ])))
        .index();
    assert_eq!(code_of(&exists_over_tag_entry(draft, site)), "image.table");
}

#[test]
fn a_branch_path_naming_a_nonexistent_hop_rejects_at_the_table_phase() {
    // A forged path whose second hop names a placement that is no branch of `notes` names
    // no reconstructed durable node, so the site is refused at the table phase; an
    // out-of-range branch hop can never resolve to — and mis-address — a durable operation.
    let mut draft = nested_branch_draft();
    let site = draft
        .add_site(SiteDef::whole_payload(nested_path(&[
            step(SemanticStepKind::Placement, 0x30),
            step(SemanticStepKind::Placement, 0x99),
        ])))
        .index();
    assert_eq!(code_of(&exists_over_tag_entry(draft, site)), "image.table");
}

// --- Composite-key admission hostiles (E03w slice C) ---
//
// A composite-key root addresses each entry by its whole ordered key tuple. The verifier
// derives the expected key-path — one column per key column, in order — from the sealed
// schema and type-checks the operand stack against it, so a forged opcode key-path that is
// too short, or whose columns are the wrong type or transposed to the wrong type, is
// refused during per-function typing. A same-typed transposition is a distinct valid
// program the physical layout distinguishes (see the composite kernel and source tests);
// the verifier catches only the count/type skews here.

/// A flat-executable root `^cells(row: int, col: text)` — a two-column composite key of
/// distinct column types, so a transposed key-path is type-detectable — with one required
/// int field `v`. Returns the draft and the whole-entry site index.
fn composite_root_draft() -> (ImageDraft, u16) {
    let mut draft = ImageDraft::new();
    let cell = draft.intern_string("Cell");
    let v = draft.intern_string("v");
    let record = draft.add_record_type(RecordTypeDef {
        name: cell,
        fields: vec![FieldDef {
            name: v,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let root = draft.intern_string("cells");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes([0x0c; 16]),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes([0x0e; 16]),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    let entry = draft.add_site(SiteDef::whole_payload(root_path())).index();
    (draft, entry)
}

/// Encode a read-only `has` export over the composite `entry` site whose body pushes the
/// given key locals (by type) then runs `DurExists`. `params` types the locals the body
/// reads; a correct call pushes `[Int, Text]` (row then col, root-first).
fn composite_exists_export(mut draft: ImageDraft, entry: u16, params: Vec<ImageType>) -> Vec<u8> {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("has");
    let mut code: Vec<Instr> = (0..params.len() as u16).map(Instr::LocalGet).collect();
    code.push(Instr::DurExists(entry));
    code.push(Instr::Return);
    let local_count = params.len() as u16;
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params,
        ret: ImageType::scalar(Scalar::Bool),
        local_count,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "has"), func);
    draft.encode().unwrap().bytes
}

#[test]
fn a_composite_root_opcode_with_the_full_ordered_key_path_verifies() {
    // The positive control: pushing both columns in order (row:int, col:text) type-checks
    // the two-column key-path and verifies.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(
        draft,
        entry,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn a_composite_root_opcode_with_a_truncated_key_path_rejects() {
    // Only one key column pushed where the two-column composite key-path needs two: the
    // operand stack cannot satisfy the derived key-path, refused at per-function typing.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(draft, entry, vec![ImageType::scalar(Scalar::Int)]);
    assert_eq!(code_of(&bytes), "image.function");
}

#[test]
fn a_composite_root_opcode_with_transposed_column_types_rejects() {
    // The columns pushed in the wrong order (text then int, where int then text is
    // required): the key-path type check fails, so a transposed key-path over distinctly
    // typed columns cannot mis-address a durable operation.
    let (draft, entry) = composite_root_draft();
    let bytes = composite_exists_export(
        draft,
        entry,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
    );
    assert_eq!(code_of(&bytes), "image.function");
}

#[test]
fn a_bounded_traversal_over_a_composite_keyed_root_layer_rejects() {
    // Bounded traversal iterates a single key column. A forged image that drives
    // `DurIterateBounded` over a composite-keyed root (two key columns), bypassing the
    // compiler's own park, is refused during per-function typing: the verifier computes the
    // traversed layer's arity from the schema and rejects any layer that is not
    // single-column, so no composite-key traversal reaches the kernel.
    let (mut draft, entry) = composite_root_draft();
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .index();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("iter");
    let code = vec![
        Instr::DurIterateBounded {
            site: entry,
            limit: 2,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
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
    draft.add_export(ExportId::of_local("", "iter"), func);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}
