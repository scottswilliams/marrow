//! Known-answer tests for the four canonical orders the encoder emits.
//!
//! Each section pin elsewhere freezes a length or a digest, which a same-width
//! permutation of rows could survive; these tests assert the exact emitted row order for
//! a small fixture whose insertion order deliberately disagrees with the canonical
//! order, so the sort itself is the thing pinned:
//!
//! - STRINGS (0x01): entries byte-sorted;
//! - CONSTS (0x04): entries by `(tag, wire-byte)` sort key, where an int's key is its
//!   big-endian two's-complement spelling — so `-1` sorts *after* `0`;
//! - EXPORTS (0x06): entries ascending by the 32 `ExportId` bytes;
//! - TEST-ENTRY (0x08): entries ascending by the remapped (byte-sorted) name index.

use marrow_image::{
    DraftTxn, EncodedImage, ExportId, FunctionDef, ImageDraft, ImageType, Instr, StrId,
};

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
}

/// The body of section `id` in `image`: the container is `magic(4) ‖ version(1) ‖
/// image-id(32) ‖ section-count(1)` followed by `id(1) ‖ len(u32) ‖ body` sections.
fn section(image: &EncodedImage, id: u8) -> Vec<u8> {
    let bytes = &image.bytes;
    let mut at = 38;
    while at < bytes.len() {
        let tag = bytes[at];
        let len =
            u32::from_be_bytes(bytes[at + 1..at + 5].try_into().expect("length prefix")) as usize;
        let body = &bytes[at + 5..at + 5 + len];
        if tag == id {
            return body.to_vec();
        }
        at += 5 + len;
    }
    panic!("section {id:#04x} is absent");
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// A minimal structurally valid function whose name and source are already interned.
fn function(name: StrId, source: StrId) -> FunctionDef {
    FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: Vec::new(),
        code: vec![Instr::Return],
    }
}

/// Strings are emitted byte-sorted, whatever order interned them.
#[test]
fn the_string_pool_is_emitted_byte_sorted() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft.intern_string("pear").expect("a within-domain mint");
    draft.intern_string("apple").expect("a within-domain mint");
    draft.intern_string("mango").expect("a within-domain mint");
    let image = draft.encode().expect("a tiny draft encodes");

    let mut expected = Vec::new();
    push_u16(&mut expected, 3);
    for text in ["apple", "mango", "pear"] {
        push_u16(&mut expected, text.len() as u16);
        expected.extend_from_slice(text.as_bytes());
    }
    assert_eq!(section(&image, 0x01), expected);
}

/// Constants are emitted by `(tag, wire-byte)` sort key. The two's-complement case is
/// load-bearing: `Int(-1)` spells `0xff…ff`, so it sorts after `Int(0)` and `Int(5)`
/// even though it is numerically smallest; bool and text tags follow every int.
#[test]
fn the_const_pool_is_emitted_in_wire_byte_order() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft.intern_text("a").expect("a within-domain mint");
    draft.intern_int(-1).expect("a within-domain mint");
    draft.intern_bool(true).expect("a within-domain mint");
    draft.intern_int(5).expect("a within-domain mint");
    draft.intern_int(0).expect("a within-domain mint");
    let image = draft.encode().expect("a tiny draft encodes");

    let mut expected = Vec::new();
    push_u16(&mut expected, 5);
    for int in [0i64, 5, -1] {
        expected.push(0x01);
        expected.extend_from_slice(&int.to_be_bytes());
    }
    expected.push(0x02);
    expected.push(0x01);
    expected.push(0x03);
    // The one interned string, at sorted index 0.
    push_u16(&mut expected, 0);
    assert_eq!(section(&image, 0x04), expected);
}

/// Exports are emitted ascending by their 32 id bytes. The fixture computes the ids,
/// inserts the entries in descending id order, and asserts the emitted rows come back
/// ascending with each id still beside the function it was bound to.
#[test]
fn the_export_table_is_emitted_in_ascending_id_order() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let mut named: Vec<(ExportId, &str)> = ["a", "b", "c"]
        .into_iter()
        .map(|item| (ExportId::of_local("", item), item))
        .collect();
    named.sort_by(|left, right| left.0.bytes().cmp(right.0.bytes()));

    // Insert in descending id order, so emission order can only come from the sort.
    let mut bound = Vec::new();
    for (id, item) in named.iter().rev() {
        let name = draft.intern_string(item).expect("a within-domain mint");
        let func = draft
            .add_function(function(name, source))
            .expect("every site operand is live");
        draft.add_export(*id, func);
        bound.push((*id, func.index()));
    }
    bound.reverse();
    let image = draft.encode().expect("a tiny draft encodes");

    let mut expected = Vec::new();
    push_u16(&mut expected, 3);
    for (id, func) in &bound {
        expected.extend_from_slice(id.bytes());
        push_u16(&mut expected, *func);
    }
    assert_eq!(section(&image, 0x06), expected);
}

/// Test entries are emitted ascending by the remapped name-string index — byte order of
/// the name text once the pool is sorted — whatever order registered them.
#[test]
fn the_test_entry_table_is_emitted_in_ascending_name_order() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let source = draft.intern_string("s").expect("a within-domain mint");
    let mut funcs = Vec::new();
    for text in ["zeta", "alpha", "mid"] {
        let name = draft.intern_string(text).expect("a within-domain mint");
        let func = draft
            .add_function(function(name, source))
            .expect("every site operand is live");
        draft.add_test_entry(name, func);
        funcs.push(func.index());
    }
    let image = draft.encode().expect("a tiny draft encodes");

    // The sorted pool is [alpha, mid, s, zeta], so the remapped name indexes are
    // alpha=0, mid=1, zeta=3, and the entries come back alphabetical: the functions
    // registered as [zeta, alpha, mid] are emitted as [alpha, mid, zeta].
    let mut expected = Vec::new();
    push_u16(&mut expected, 3);
    for (name_index, func) in [(0u16, funcs[1]), (1, funcs[2]), (3, funcs[0])] {
        push_u16(&mut expected, name_index);
        push_u16(&mut expected, func);
    }
    assert_eq!(section(&image, 0x08), expected);
}
