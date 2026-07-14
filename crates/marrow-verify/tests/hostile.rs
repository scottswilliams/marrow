//! Slice K.4 hostile-image evidence (design §E, finding 9), two suites:
//!
//! (a) *stale-digest*: byte flips over a good image with no rehash — every one must
//!     reject at phase 1 (the digest no longer matches the payload).
//! (b) *structured single-invariant*: artifacts that each violate exactly one later
//!     invariant, carrying a valid (recomputed or encoder-computed) digest — each
//!     must reject at the phase that owns that invariant.
//!
//! Semantically valid rewrites are allowed to verify and are not asserted to reject.

use marrow_image::{FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, image_id};
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
    draft.add_export(main_name, main);
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
    // EXPORTS is section id 6: body = count(u16), then per export name(u16) func(u16).
    let (_, body, _) = *sections(&bytes).iter().find(|(id, ..)| *id == 6).unwrap();
    let func_field = body + 2 + 2; // after count and the first export's name
    bytes[func_field] = 0xFF;
    bytes[func_field + 1] = 0xFF;
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
    draft.add_export(name, func);
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
    draft.add_export(main_name, main);
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
    draft.add_export(ping_name, ping);
    assert_eq!(code_of(&draft.encode().unwrap().bytes), "image.closure");
}
