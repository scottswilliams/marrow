//! Hostile stored-leaf names. A stored struct leaf and an enum payload leaf are spelled
//! `0x03 ‖ u16(name_len) ‖ name ‖ value` in the DURABLE section, and the name is part of
//! the durable contract. The verifier refuses a leaf position that does not start with
//! the marker (the name-free grammar placed a value tag there), a name that is empty,
//! truncated, over the string bound or not UTF-8, a struct leaf name that disagrees with
//! the record the VM reads, and a second occurrence of an enum whose payload names differ
//! from its first.

use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, EnumTypeDef, ExportId, FieldDef,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, ValueShapeEnumMember, ValueShapeLeaf,
    ValueShapeNodeId, VariantDef,
};
use marrow_test_support::{admitted_plan, rehash, site};
use marrow_verify::{Bound, Region, RejectionKind, Tag, TieFault, TieNode, VerifyPhase};

use super::{refusal_of, section_frame};

const SUM_SHAPE: [u8; 16] = [0x50; 16];
const MEMBER_RECT: [u8; 16] = [0x51; 16];
const SUM_PLACE: [u8; 16] = [0x52; 16];
const MEMBER_AT: [u8; 16] = [0x53; 16];

fn id(bytes: [u8; 16]) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes(bytes)
}

fn leaves(names: &[&str], shape: ValueShapeNodeId) -> Vec<ValueShapeLeaf> {
    names
        .iter()
        .map(|name| ValueShapeLeaf::new(*name, shape))
        .collect()
}

/// The image-side types every stored value below is checked against: `struct Pos { x:
/// int, y: int }`, `enum Shape { rect(int, int) }`, and `enum Place { at(Pos) }`.
struct Types {
    pos: ImageType,
    shape: ImageType,
    place: ImageType,
}

fn declare_types(draft: &mut DraftTxn<'_>) -> Types {
    let int = ImageType::scalar(Scalar::Int);
    let mut name = |text: &str| draft.intern_string(text).expect("a within-domain mint");
    let (pos_name, x, y) = (name("Pos"), name("x"), name("y"));
    let (shape_name, rect) = (name("Shape"), name("rect"));
    let (place_name, at) = (name("Place"), name("at"));
    let pos = draft
        .add_record_type(RecordTypeDef {
            name: pos_name,
            fields: vec![
                FieldDef {
                    name: x,
                    ty: int,
                    required: true,
                },
                FieldDef {
                    name: y,
                    ty: int,
                    required: true,
                },
            ],
        })
        .expect("a within-domain mint");
    let pos = ImageType::Record {
        idx: pos,
        optional: false,
    };
    let shape = draft
        .add_enum_type(EnumTypeDef {
            name: shape_name,
            variants: vec![VariantDef {
                name: rect,
                category: false,
                payload: vec![int, int],
            }],
        })
        .expect("a within-domain mint");
    let place = draft
        .add_enum_type(EnumTypeDef {
            name: place_name,
            variants: vec![VariantDef {
                name: at,
                category: false,
                payload: vec![pos],
            }],
        })
        .expect("a within-domain mint");
    Types {
        pos,
        shape: ImageType::Enum {
            idx: shape,
            optional: false,
        },
        place: ImageType::Enum {
            idx: place,
            optional: false,
        },
    }
}

/// A one-root image whose `Marker` record stores one required field per entry of
/// `fields`, each with the record type and the durable value shape `fields` mints.
fn build(
    fields: impl FnOnce(&mut DraftTxn<'_>, &Types) -> Vec<(ImageType, ValueShapeNodeId)>,
) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    draft.set_application_identity(id([0x0a; 16]));
    let types = declare_types(&mut draft);
    let fields = fields(&mut draft, &types);
    let mut record_fields = Vec::new();
    for (ordinal, (ty, _)) in fields.iter().enumerate() {
        let name = draft
            .intern_string(&format!("f{ordinal}"))
            .expect("a within-domain mint");
        record_fields.push(FieldDef {
            name,
            ty: *ty,
            required: true,
        });
    }
    let record_name = draft.intern_string("Marker").expect("a within-domain mint");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: record_fields,
        })
        .expect("a within-domain mint");
    let members = fields
        .iter()
        .enumerate()
        .map(|(ordinal, (_, value))| DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: id([0x10 + ordinal as u8; 16]),
                required: true,
                value: *value,
            },
        })
        .collect();
    draft
        .declare_product(&admitted_plan(), id([0x0d; 16]), record, members)
        .expect("a well-formed declaration");
    let root_name = draft
        .intern_string("markers")
        .expect("a within-domain mint");
    let occurrence = draft
        .add_root_occurrence(
            &admitted_plan(),
            id([0x0d; 16]),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: id([0x0c; 16]),
                }],
                placement: id([0x0b; 16]),
                indexes: vec![].into(),
            },
        )
        .expect("the Product is declared");
    site(
        &mut draft,
        occurrence.occurrence(),
        occurrence.placement_path(),
        SemanticTarget::WholePayload,
    );
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let fname = draft.intern_string("f").expect("a within-domain mint");
    let code = vec![Instr::LocalGet(0), Instr::Return];
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft
        .add_function(FunctionDef {
            name: fname,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::scalar(Scalar::Int),
            local_count: 1,
            spans,
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

/// A `Pos` value whose leaves carry `names`.
fn pos_named(names: &'static [&'static str]) -> Vec<u8> {
    build(move |draft, types| {
        let int = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        let pos = draft
            .value_struct(leaves(names, int))
            .expect("a within-bounds shape appends");
        vec![(types.pos, pos)]
    })
}

/// A `Place::at(pos)` value whose nested `Pos` leaves carry `names`.
fn place_named(names: &'static [&'static str]) -> Vec<u8> {
    build(move |draft, types| {
        let int = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        let pos = draft
            .value_struct(leaves(names, int))
            .expect("a within-bounds shape appends");
        let place = draft
            .value_enum(
                id(SUM_PLACE),
                vec![ValueShapeEnumMember::new(
                    id(MEMBER_AT),
                    vec![ValueShapeLeaf::new("pos", pos)],
                )],
            )
            .expect("a within-bounds shape appends");
        vec![(types.place, place)]
    })
}

/// Two fields of one enum `Shape`, the second presenting `second` payload names.
fn two_shapes(second: &'static [&'static str]) -> Vec<u8> {
    build(move |draft, types| {
        let int = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        let mut shape = |names: &[&str]| {
            draft
                .value_enum(
                    id(SUM_SHAPE),
                    vec![ValueShapeEnumMember::new(
                        id(MEMBER_RECT),
                        leaves(names, int),
                    )],
                )
                .expect("a within-bounds shape appends")
        };
        let first = shape(&["width", "height"]);
        let second = shape(second);
        vec![(types.shape, first), (types.shape, second)]
    })
}

/// Replace the first occurrence of `original` in the DURABLE section with `forged`,
/// repair the section's length, and revalidate the digest.
fn forge_durable(bytes: &mut Vec<u8>, original: &[u8], forged: &[u8]) {
    let (body, len) = section_frame(bytes, 3);
    let at = bytes[body..body + len]
        .windows(original.len())
        .position(|window| window == original)
        .map(|offset| body + offset)
        .expect("the bytes are present in the durable section");
    bytes.splice(at..at + original.len(), forged.iter().copied());
    let forged_len = (len - original.len() + forged.len()) as u32;
    bytes[body - 4..body].copy_from_slice(&forged_len.to_be_bytes());
    rehash(bytes);
}

/// The first `Pos` leaf as the encoder spells it: marker, length 1, `x`, scalar int.
const LEAF_X: &[u8] = &[0x03, 0x00, 0x01, b'x', 0x00, 0x01];

fn table(kind: RejectionKind) -> Option<(VerifyPhase, RejectionKind)> {
    Some((VerifyPhase::Table, kind))
}

#[test]
fn well_named_stored_leaves_verify() {
    for (what, bytes) in [
        ("struct", pos_named(&["x", "y"])),
        ("struct in payload", place_named(&["x", "y"])),
        ("enum reoccurrence", two_shapes(&["width", "height"])),
    ] {
        assert_eq!(refusal_of(&bytes), None, "{what}");
    }
}

/// The name-free grammar started every leaf with a value tag from `{0, 1, 2}`; at a leaf
/// position that byte is not the marker, so such an image can never be read as though
/// its leaves were named.
#[test]
fn a_durable_leaf_without_its_name_marker_is_rejected() {
    for tag in [0x00, 0x01, 0x02] {
        let mut bytes = pos_named(&["x", "y"]);
        let mut forged = LEAF_X.to_vec();
        forged[0] = tag;
        forge_durable(&mut bytes, LEAF_X, &forged);
        assert_eq!(
            refusal_of(&bytes),
            table(RejectionKind::Unknown(Tag::DurableLeaf)),
            "value tag {tag}"
        );
    }
}

/// A struct leaf's name must be the name of the record field the VM reads at that
/// position, at the top level and inside an enum payload.
#[test]
fn a_durable_struct_leaf_name_disagreeing_with_its_record_is_rejected() {
    let mismatch = table(RejectionKind::RecordTie {
        node: TieNode::Root,
        fault: TieFault::FieldMismatch,
    });
    for (what, bytes) in [
        ("renamed", pos_named(&["x", "z"])),
        ("swapped", pos_named(&["y", "x"])),
        ("renamed in payload", place_named(&["x", "z"])),
        ("swapped in payload", place_named(&["y", "x"])),
    ] {
        assert_eq!(refusal_of(&bytes), mismatch, "{what}");
    }
}

#[test]
fn a_durable_leaf_name_that_is_empty_truncated_overlong_or_not_utf8_is_rejected() {
    let over = (marrow_image::bounds::MAX_STRING_BYTES + 1) as u16;
    let cases: [(&str, Vec<u8>, RejectionKind); 4] = [
        (
            "empty",
            vec![0x03, 0x00, 0x00, 0x00, 0x01],
            RejectionKind::EmptyLeafName,
        ),
        (
            "truncated",
            vec![0x03, 0x0f, 0xff, b'x', 0x00, 0x01],
            RejectionKind::Truncated(Region::Durable),
        ),
        (
            "over the string bound",
            [&[0x03][..], &over.to_be_bytes(), &[b'x', 0x00, 0x01]].concat(),
            RejectionKind::OverBound(Bound::StringBytes),
        ),
        (
            "not UTF-8",
            vec![0x03, 0x00, 0x01, 0xff, 0x00, 0x01],
            RejectionKind::InvalidUtf8,
        ),
    ];
    for (what, forged, kind) in cases {
        let mut bytes = pos_named(&["x", "y"]);
        forge_durable(&mut bytes, LEAF_X, &forged);
        assert_eq!(refusal_of(&bytes), table(kind), "{what}");
    }
}

/// A second field of one enum is a reference to its first occurrence's identity, which
/// includes each member's payload names in order.
#[test]
fn an_enum_reoccurrence_with_different_payload_names_is_rejected() {
    for (what, second) in [
        ("swapped", &["height", "width"][..]),
        ("renamed", &["wide", "height"][..]),
    ] {
        assert_eq!(
            refusal_of(&two_shapes(second)),
            table(RejectionKind::EnumIdentityReused),
            "{what}"
        );
    }
}
