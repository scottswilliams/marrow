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
    DurableMemberDef, EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef, ImageDraft, ImageType,
    Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar, SiteDef,
    SiteTarget, SpanEntry, VariantDef, image_id,
};
use marrow_verify::verify;

/// The tracer `Counter` record's durable member tree: `value:int` required then
/// `label:string` sparse, matching the `durable_schema` record fields so the
/// verifier's member-tree/record cross-check passes.
fn counters_members() -> Vec<DurableMemberDef> {
    vec![
        DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x0e; 16]),
            scalar: Scalar::Int,
            required: true,
        },
        DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes([0x0f; 16]),
            scalar: Scalar::Text,
            required: false,
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
            members: counters_members(),
        },
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
fn executable_site_over_a_composite_root_rejects() {
    // A composite-key root carries a complete identity but is not executable: the
    // single-root kernel serves only single-column keyed roots. A forged image that
    // adds an operation site over a two-column root is refused independently during
    // flow validation, regardless of the compiler's own boundary.
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
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes([0x0e; 16]),
                scalar: Scalar::Int,
                required: true,
            }],
        },
    });
    let value_site = draft.add_site(SiteDef {
        root: 0,
        target: SiteTarget::Field(0),
    });
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
    // The composite-key root is rejected during per-function structural/type
    // recording (the `image.function` phase), where the operation site is typed.
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.function");
}

/// Build a `Book { title:string required }` root at `^books(id:int)` whose durable
/// member tree adds a static `details` group (holding `pages:int`) and a keyed
/// `notes(noteId:string)` branch (holding `text:string required`). The record
/// carries only the top-level `title` field, so the verifier's member-tree/record
/// cross-check passes. When `with_site` is true, an operation site and a reading
/// function are added — the not-yet-executable shape a forged image would need.
fn group_branch_draft(with_site: bool) -> ImageDraft {
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
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    scalar: Scalar::Text,
                    required: true,
                },
                DurableMemberDef::Group {
                    id: LedgerIdBytes::from_bytes([0x20; 16]),
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x21; 16]),
                        scalar: Scalar::Int,
                        required: false,
                    }],
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes([0x30; 16]),
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x31; 16]),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x32; 16]),
                        scalar: Scalar::Text,
                        required: true,
                    }],
                },
            ],
        },
    });
    let src = draft.intern_string("src/main.mw");
    if with_site {
        let site = draft.add_site(SiteDef {
            root: 0,
            target: SiteTarget::Field(0),
        });
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
    // A single-column keyed root whose resource declares a group or branch carries a
    // complete identity but is not yet executable. A forged image that adds an
    // operation site over it is refused independently during per-function typing
    // (the `image.function` phase), regardless of the compiler's own boundary.
    assert_eq!(
        code_of(&group_branch_draft(true).encode().unwrap().bytes),
        "image.function"
    );
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

#[test]
fn durable_root_with_a_non_scalar_field_rejects() {
    // A durable root record carrying an enum-valued field has no store
    // representation, so the durable table rejects it.
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let rname = draft.intern_string("R");
    let ename = draft.intern_string("E");
    let vname = draft.intern_string("only");
    let idn = draft.intern_string("id");
    let tagn = draft.intern_string("tag");
    draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: vname,
            category: false,
            payload: Vec::new(),
        }],
    });
    let rec = draft.add_record_type(RecordTypeDef {
        name: rname,
        fields: vec![
            FieldDef {
                name: idn,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: tagn,
                ty: ImageType::Enum {
                    idx: 0,
                    optional: false,
                },
                required: false,
            },
        ],
    });
    let root = draft.intern_string("boxes");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x1a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x1c; 16]),
        }],
        record: rec,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x1b; 16]),
            product: LedgerIdBytes::from_bytes([0x1d; 16]),
            // The durable member tree carries only the scalar field; the enum field
            // has no durable representation and is what makes verification reject.
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes([0x1e; 16]),
                scalar: Scalar::Int,
                required: true,
            }],
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
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.table");
}
