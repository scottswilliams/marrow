//! C03 collection verification evidence: a well-formed `List`/`Map` image verifies
//! and seals, and each single-invariant hostile image rejects at the phase that owns
//! the violated collection invariant. Built through `ImageDraft` (encoder-computed
//! digest), so every rejection is a structural/type invariant, not a digest flip.

use marrow_image::{
    CollectionTypeDef, DraftTxn, ExportId, FunctionDef, ImageBuildError, ImageDraft, ImageType,
    Instr, Scalar, SpanEntry,
};
use marrow_verify::verify;

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
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

/// Build a single-export image whose `main` body is `code`, returning `ret`, over a
/// caller-supplied COLLTYPES table.
fn image_with(colls: &[CollectionTypeDef], code: Vec<Instr>, ret: ImageType) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    for coll in colls {
        draft
            .add_collection_type(*coll)
            .expect("a within-domain mint");
    }
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret,
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().expect("encode").bytes
}

const LIST_INT: CollectionTypeDef = CollectionTypeDef::List {
    elem: ImageType::Scalar {
        scalar: Scalar::Int,
        optional: false,
    },
};

const MAP_STR_INT: CollectionTypeDef = CollectionTypeDef::Map {
    key: ImageType::Scalar {
        scalar: Scalar::Text,
        optional: false,
    },
    value: ImageType::Scalar {
        scalar: Scalar::Int,
        optional: false,
    },
};

#[test]
fn a_well_formed_list_program_verifies_and_seals() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let five = draft.intern_int(5).expect("a within-domain mint");
    let code = vec![
        Instr::ListNew(marrow_image::CollTypeId::from_index(0)),
        Instr::ConstLoad(five),
        Instr::ListAppend,
        Instr::ListLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let verified = verify(&bytes).expect("a well-formed list image verifies");
    assert_eq!(verified.collections().len(), 1);
}

#[test]
fn a_well_formed_map_program_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(MAP_STR_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let code = vec![
        Instr::MapNew(marrow_image::CollTypeId::from_index(0)),
        Instr::MapLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).expect("a well-formed map image verifies");
}

/// Flipped by the coherence hoist (the pinned flip lives in `legacy_ok_pins.rs`): an
/// out-of-range `ListNew` is refused by the producer, so no image carrying one can be
/// emitted for the verifier to see. The wrong-KIND case below stays the verifier's
/// own function-phase rejection — its ordinal is in range, so it still encodes.
#[test]
fn a_list_new_index_out_of_range_is_refused_by_the_producer() {
    // Only one collection type exists, so `ListNew(9)` names no collection.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let code = vec![
        Instr::ListNew(marrow_image::CollTypeId::from_index(9)),
        Instr::ListLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

#[test]
fn a_map_op_on_a_list_type_rejects() {
    // `MapNew(0)` names a list collection type, not a map.
    let bytes = image_with(
        &[LIST_INT],
        vec![
            Instr::MapNew(marrow_image::CollTypeId::from_index(0)),
            Instr::MapLen,
            Instr::Return,
        ],
        ImageType::scalar(Scalar::Int),
    );
    let rejection = verify(&bytes).expect_err("a map op on a list type rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_list_append_element_type_mismatch_rejects() {
    // Appending a bool to a `List[int]` is a per-opcode type violation.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let flag = draft.intern_bool(true).expect("a within-domain mint");
    let code = vec![
        Instr::ListNew(marrow_image::CollTypeId::from_index(0)),
        Instr::ConstLoad(flag),
        Instr::ListAppend,
        Instr::ListLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let rejection = verify(&bytes).expect_err("a list-append type mismatch rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_map_key_that_is_not_a_scalar_rejects() {
    // A map whose key type is a collection reference is not an admitted key type.
    let bad_map = CollectionTypeDef::Map {
        key: ImageType::Collection {
            idx: marrow_image::CollTypeId::from_index(0),
            optional: false,
        },
        value: ImageType::scalar(Scalar::Int),
    };
    // Row 0 is a valid list; row 1 is the bad map (its key references row 0).
    let bytes = image_with(&[LIST_INT, bad_map], vec![Instr::Return], ImageType::Unit);
    let rejection = verify(&bytes).expect_err("a non-scalar map key rejects");
    assert_eq!(rejection.code(), "image.table");
}

const LIST_STR: CollectionTypeDef = CollectionTypeDef::List {
    elem: ImageType::Scalar {
        scalar: Scalar::Text,
        optional: false,
    },
};

#[test]
fn a_well_formed_text_split_join_program_verifies() {
    // `join(split(text, sep), sep)` over a `List[string]`: split consumes two texts
    // and yields the list, join consumes the list and a text and yields a text.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_STR)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let hay = draft.intern_text("a,b,c").expect("a within-domain mint");
    let sep = draft.intern_text(",").expect("a within-domain mint");
    let code = vec![
        Instr::ConstLoad(hay),
        Instr::ConstLoad(sep),
        Instr::TextSplit(marrow_image::CollTypeId::from_index(0)),
        Instr::ConstLoad(sep),
        Instr::TextJoin,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Text),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).expect("a well-formed split/join image verifies");
}

#[test]
fn a_text_split_naming_a_non_string_list_rejects() {
    // `TextSplit(0)` names a `List[int]`, but the text floor produces only a
    // `List[string]`; the hostile image is rejected rather than run.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let hay = draft.intern_text("a,b").expect("a within-domain mint");
    let sep = draft.intern_text(",").expect("a within-domain mint");
    let code = vec![
        Instr::ConstLoad(hay),
        Instr::ConstLoad(sep),
        Instr::TextSplit(marrow_image::CollTypeId::from_index(0)),
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::Collection {
                idx: marrow_image::CollTypeId::from_index(0),
                optional: false,
            },
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let rejection = verify(&bytes).expect_err("split naming a List[int] rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_text_join_on_a_non_string_list_rejects() {
    // `TextJoin` requires a `List[string]`; a `List[int]` operand is rejected.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft
        .add_collection_type(LIST_INT)
        .expect("a within-domain mint");
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let sep = draft.intern_text(",").expect("a within-domain mint");
    let code = vec![
        Instr::ListNew(marrow_image::CollTypeId::from_index(0)),
        Instr::ConstLoad(sep),
        Instr::TextJoin,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Text),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let rejection = verify(&bytes).expect_err("join on a List[int] rejects");
    assert_eq!(rejection.code(), "image.function");
}
