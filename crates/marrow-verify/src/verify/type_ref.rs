//! The one type-reference decoder.
//!
//! Every type the container spells is the same one-byte tag (optionally flagged) plus, for
//! a table reference, a big-endian `u16` index. Positions differ only in which tags they
//! admit, whether the optional flag is admitted, and which tables bound the index;
//! [`TypeRules`] states those three things, and this module is the only reader of them.

use super::reject;
use super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{RejectionKind, TypePosition, TypeRefFault, VerifyPhase, VerifyRejection};
use marrow_image::{
    CollTypeId, EnumId, ImageType, OPTIONAL_FLAG, RootId, TAG_BOOL, TAG_BYTES, TAG_COLLECTION,
    TAG_DATE, TAG_DURATION, TAG_ENUM, TAG_IDENTITY, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT,
    TAG_UNIT, TypeId,
};

/// The type kinds one position admits.
#[derive(Clone, Copy)]
pub(super) struct TagSet(u8);

impl TagSet {
    pub(super) const UNIT: Self = Self(1 << 0);
    pub(super) const SCALAR: Self = Self(1 << 1);
    pub(super) const RECORD: Self = Self(1 << 2);
    pub(super) const ENUM: Self = Self(1 << 3);
    pub(super) const COLLECTION: Self = Self(1 << 4);
    pub(super) const IDENTITY: Self = Self(1 << 5);
    /// The value types: a scalar, record, enum, or collection.
    pub(super) const VALUE: Self = Self::SCALAR
        .with(Self::RECORD)
        .with(Self::ENUM)
        .with(Self::COLLECTION);

    pub(super) const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn admits(self, kind: Self) -> bool {
        self.0 & kind.0 != 0
    }
}

/// Whether a position admits the optional flag on its tag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Optionality {
    /// Bare only: a flagged tag is a malformed image.
    Bare,
    /// Either spelling.
    Either,
    /// Optional only: an unflagged tag is a malformed image.
    Optional,
}

/// One type-reference position: the phase and position a rejection from it carries, the
/// tags it admits, its optionality, and the exclusive table bound each referenced index
/// must fall inside. A `None` bound leaves that index for a later pass to range-check,
/// which is what a forward reference into a table this one precedes needs.
pub(super) struct TypeRules {
    phase: VerifyPhase,
    position: TypePosition,
    allowed: TagSet,
    optionality: Optionality,
    types: Option<usize>,
    enums: Option<usize>,
    collections: Option<usize>,
    roots: Option<usize>,
}

impl TypeRules {
    /// A table-phase position with every referenced index left unchecked.
    pub(super) const fn new(
        position: TypePosition,
        allowed: TagSet,
        optionality: Optionality,
    ) -> Self {
        Self {
            phase: VerifyPhase::Table,
            position,
            allowed,
            optionality,
            types: None,
            enums: None,
            collections: None,
            roots: None,
        }
    }

    pub(super) const fn in_phase(mut self, phase: VerifyPhase) -> Self {
        self.phase = phase;
        self
    }

    pub(super) const fn types(mut self, count: usize) -> Self {
        self.types = Some(count);
        self
    }

    pub(super) const fn enums(mut self, count: usize) -> Self {
        self.enums = Some(count);
        self
    }

    pub(super) const fn collections(mut self, count: usize) -> Self {
        self.collections = Some(count);
        self
    }

    pub(super) const fn roots(mut self, count: usize) -> Self {
        self.roots = Some(count);
        self
    }

    fn reject(&self, fault: TypeRefFault) -> VerifyRejection {
        reject(
            self.phase,
            RejectionKind::TypeRef {
                position: self.position,
                fault,
            },
        )
    }
}

/// Decode one type reference under `rules`.
pub(super) fn decode_type_ref(
    reader: &mut Reader,
    rules: &TypeRules,
) -> Result<ImageType, VerifyRejection> {
    let tag = reader
        .u8()
        .ok_or_else(|| rules.reject(TypeRefFault::Truncated))?;
    let optional = tag & OPTIONAL_FLAG != 0;
    let spelling_admitted = match rules.optionality {
        Optionality::Bare => !optional,
        Optionality::Either => true,
        Optionality::Optional => optional,
    };
    // The unit type has no vacant form, wherever the flag is otherwise admitted.
    let base = tag & !OPTIONAL_FLAG;
    if !spelling_admitted || (base == TAG_UNIT && optional) {
        return Err(rules.reject(TypeRefFault::Optionality));
    }
    let kind = match base {
        TAG_UNIT => TagSet::UNIT,
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => {
            TagSet::SCALAR
        }
        TAG_RECORD => TagSet::RECORD,
        TAG_ENUM => TagSet::ENUM,
        TAG_COLLECTION => TagSet::COLLECTION,
        TAG_IDENTITY => TagSet::IDENTITY,
        _ => return Err(rules.reject(TypeRefFault::TagNotAdmitted)),
    };
    if !rules.allowed.admits(kind) {
        return Err(rules.reject(TypeRefFault::TagNotAdmitted));
    }
    Ok(match base {
        TAG_UNIT => ImageType::Unit,
        TAG_RECORD => ImageType::Record {
            idx: TypeId::from_index(index(reader, rules, rules.types)?),
            optional,
        },
        TAG_ENUM => ImageType::Enum {
            idx: EnumId::from_index(index(reader, rules, rules.enums)?),
            optional,
        },
        TAG_COLLECTION => ImageType::Collection {
            idx: CollTypeId::from_index(index(reader, rules, rules.collections)?),
            optional,
        },
        TAG_IDENTITY => ImageType::Identity {
            root: RootId::from_index(index(reader, rules, rules.roots)?),
            optional,
        },
        _ => ImageType::Scalar {
            scalar: decode_bare_scalar(base).expect("a scalar base tag"),
            optional,
        },
    })
}

/// One table-reference index, range-checked against `limit` when the position states
/// one.
fn index(
    reader: &mut Reader,
    rules: &TypeRules,
    limit: Option<usize>,
) -> Result<u16, VerifyRejection> {
    let idx = reader
        .u16()
        .ok_or_else(|| rules.reject(TypeRefFault::Truncated))?;
    match limit {
        Some(limit) if idx as usize >= limit => Err(rules.reject(TypeRefFault::IndexOutOfRange)),
        _ => Ok(idx),
    }
}
