//! C03 collection verification evidence: a well-formed `List`/`Map` image verifies
//! and seals, and each single-invariant hostile image rejects at the phase that owns
//! the violated collection invariant. Built through `ImageDraft` (encoder-computed
//! digest), so every rejection is a structural/type invariant, not a digest flip.

use marrow_image::{
    CollectionTypeDef, ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
};
use marrow_verify::verify;

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
    let mut draft = ImageDraft::new();
    for coll in colls {
        draft.add_collection_type(*coll);
    }
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret,
        local_count: 0,
        code,
        spans,
    });
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
    let mut draft = ImageDraft::new();
    draft.add_collection_type(LIST_INT);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let five = draft.intern_int(5);
    let code = vec![
        Instr::ListNew(0),
        Instr::ConstLoad(five.index()),
        Instr::ListAppend,
        Instr::ListLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let verified = verify(&bytes).expect("a well-formed list image verifies");
    assert_eq!(verified.collections().len(), 1);
}

#[test]
fn a_well_formed_map_program_verifies() {
    let mut draft = ImageDraft::new();
    draft.add_collection_type(MAP_STR_INT);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let code = vec![Instr::MapNew(0), Instr::MapLen, Instr::Return];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).expect("a well-formed map image verifies");
}

#[test]
fn a_list_new_index_out_of_range_rejects() {
    // Only one collection type exists, so `ListNew(9)` names no collection.
    let bytes = image_with(
        &[LIST_INT],
        vec![Instr::ListNew(9), Instr::ListLen, Instr::Return],
        ImageType::scalar(Scalar::Int),
    );
    let rejection = verify(&bytes).expect_err("an out-of-range list-new index rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_map_op_on_a_list_type_rejects() {
    // `MapNew(0)` names a list collection type, not a map.
    let bytes = image_with(
        &[LIST_INT],
        vec![Instr::MapNew(0), Instr::MapLen, Instr::Return],
        ImageType::scalar(Scalar::Int),
    );
    let rejection = verify(&bytes).expect_err("a map op on a list type rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_list_append_element_type_mismatch_rejects() {
    // Appending a bool to a `List[int]` is a per-opcode type violation.
    let mut draft = ImageDraft::new();
    draft.add_collection_type(LIST_INT);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let flag = draft.intern_bool(true);
    let code = vec![
        Instr::ListNew(0),
        Instr::ConstLoad(flag.index()),
        Instr::ListAppend,
        Instr::ListLen,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code,
        spans,
    });
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
            idx: 0,
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
    let mut draft = ImageDraft::new();
    draft.add_collection_type(LIST_STR);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let hay = draft.intern_text("a,b,c");
    let sep = draft.intern_text(",");
    let code = vec![
        Instr::ConstLoad(hay.index()),
        Instr::ConstLoad(sep.index()),
        Instr::TextSplit(0),
        Instr::ConstLoad(sep.index()),
        Instr::TextJoin,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Text),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).expect("a well-formed split/join image verifies");
}

#[test]
fn a_text_split_naming_a_non_string_list_rejects() {
    // `TextSplit(0)` names a `List[int]`, but the text floor produces only a
    // `List[string]`; the hostile image is rejected rather than run.
    let mut draft = ImageDraft::new();
    draft.add_collection_type(LIST_INT);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let hay = draft.intern_text("a,b");
    let sep = draft.intern_text(",");
    let code = vec![
        Instr::ConstLoad(hay.index()),
        Instr::ConstLoad(sep.index()),
        Instr::TextSplit(0),
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::Collection {
            idx: 0,
            optional: false,
        },
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let rejection = verify(&bytes).expect_err("split naming a List[int] rejects");
    assert_eq!(rejection.code(), "image.function");
}

#[test]
fn a_text_join_on_a_non_string_list_rejects() {
    // `TextJoin` requires a `List[string]`; a `List[int]` operand is rejected.
    let mut draft = ImageDraft::new();
    draft.add_collection_type(LIST_INT);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let sep = draft.intern_text(",");
    let code = vec![
        Instr::ListNew(0),
        Instr::ConstLoad(sep.index()),
        Instr::TextJoin,
        Instr::Return,
    ];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Text),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    let rejection = verify(&bytes).expect_err("join on a List[int] rejects");
    assert_eq!(rejection.code(), "image.function");
}
