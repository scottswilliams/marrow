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
    ExportId, FieldDef, FuncId, FunctionDef, ImageDraft, ImageType, Instr, RecordTypeDef, RootDef,
    Scalar, SiteDef, SiteTarget, SpanEntry, image_id,
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
fn test_entry_with_durable_demand_rejects() {
    // A test entry whose body reads durable data: demand must be empty.
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
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.test_entry");
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
            ty: Scalar::Int,
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
