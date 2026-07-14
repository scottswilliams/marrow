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
    ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr, RecordTypeDef, RootDef, Scalar,
    SiteDef, SiteTarget, SpanEntry, image_id,
};
use marrow_verify::verify;

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

/// The seven section frames as `(id, body_offset, body_len)` (header is 38 bytes).
fn sections(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut off = 38usize;
    for _ in 0..7 {
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
        params: vec![Scalar::Int],
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
                ty: Scalar::Int,
                required: true,
            },
            FieldDef {
                name: label,
                ty: Scalar::Text,
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.add_root(RootDef {
        name: root,
        key: Scalar::Text,
        record,
    });
    let entry = draft.add_site(SiteDef {
        root: 0,
        target: SiteTarget::Entry,
    });
    let value_site = draft.add_site(SiteDef {
        root: 0,
        target: SiteTarget::Field(0),
    });
    let label_site = draft.add_site(SiteDef {
        root: 0,
        target: SiteTarget::Field(1),
    });
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
        params: vec![Scalar::Text, Scalar::Int],
        ret: ImageType::Unit,
        local_count: 2,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft
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
