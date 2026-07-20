use super::*;

/// A lowered value type: a scalar, a nominal int type, or the project record,
/// each bare or optional. A nominal is int-shaped in the image; its distinct
/// check-time identity lives here and in the [`TypeRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LTy {
    Scalar {
        scalar: ScalarType,
        optional: bool,
    },
    Nominal {
        id: NominalId,
        optional: bool,
    },
    Record {
        ty: TypeId,
        optional: bool,
    },
    /// A dense `struct` value. Like [`LTy::Record`] it is image-`Record`-shaped and
    /// runtime-`Value::Record`-shaped (the one product representation owner), but
    /// it is a distinct value type: constructible and returnable, every field
    /// present. The `TypeId` names its image record def.
    Struct {
        ty: TypeId,
        optional: bool,
    },
    /// A closed enum value, image-`Enum`- and runtime-`Value::Enum`-shaped. Like
    /// the other nominal products it is a distinct value type; the `EnumId` names
    /// its image ENUMS-table entry.
    Enum {
        ty: EnumId,
        optional: bool,
    },
    /// A finite collection value (`List<T>` / `Map<K, V>`), image-`Collection`- and
    /// runtime-`Value::List`/`Value::Map`-shaped. `idx` names its image COLLTYPES
    /// entry; the source element/key/value types live in the registry's collection
    /// table.
    Collection {
        idx: u16,
        optional: bool,
    },
    /// An abstract generic type parameter, present only while the once-checked
    /// template pass lowers a generic body against a throwaway draft. `index` is the
    /// parameter's declaration position; its constraint is read from the lowerer's
    /// type environment. A monomorphized instantiation never carries a `Param`.
    Param {
        index: u16,
        optional: bool,
    },
    /// An entry identity `Id(^root)`, image-`Identity`- and runtime-`Value::Id`-shaped.
    /// `root` is the store root's ROOTS-table index (0 — a program has one root). A
    /// distinct value type: a by-value runtime/lookup value, not a durable field or key.
    Identity {
        root: u16,
        optional: bool,
    },
}

impl LTy {
    pub(super) fn bare_scalar(scalar: ScalarType) -> Self {
        LTy::Scalar {
            scalar,
            optional: false,
        }
    }

    pub(super) fn is_optional(self) -> bool {
        match self {
            LTy::Scalar { optional, .. }
            | LTy::Nominal { optional, .. }
            | LTy::Record { optional, .. }
            | LTy::Struct { optional, .. }
            | LTy::Enum { optional, .. }
            | LTy::Collection { optional, .. }
            | LTy::Param { optional, .. }
            | LTy::Identity { optional, .. } => optional,
        }
    }

    pub(super) fn to_optional(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::Scalar {
                scalar,
                optional: true,
            },
            LTy::Nominal { id, .. } => LTy::Nominal { id, optional: true },
            LTy::Record { ty, .. } => LTy::Record { ty, optional: true },
            LTy::Struct { ty, .. } => LTy::Struct { ty, optional: true },
            LTy::Enum { ty, .. } => LTy::Enum { ty, optional: true },
            LTy::Collection { idx, .. } => LTy::Collection {
                idx,
                optional: true,
            },
            LTy::Param { index, .. } => LTy::Param {
                index,
                optional: true,
            },
            LTy::Identity { root, .. } => LTy::Identity {
                root,
                optional: true,
            },
        }
    }

    pub(super) fn to_bare(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::bare_scalar(scalar),
            LTy::Nominal { id, .. } => LTy::Nominal {
                id,
                optional: false,
            },
            LTy::Record { ty, .. } => LTy::Record {
                ty,
                optional: false,
            },
            LTy::Struct { ty, .. } => LTy::Struct {
                ty,
                optional: false,
            },
            LTy::Enum { ty, .. } => LTy::Enum {
                ty,
                optional: false,
            },
            LTy::Collection { idx, .. } => LTy::Collection {
                idx,
                optional: false,
            },
            LTy::Param { index, .. } => LTy::Param {
                index,
                optional: false,
            },
            LTy::Identity { root, .. } => LTy::Identity {
                root,
                optional: false,
            },
        }
    }

    /// The abstract type-parameter index, if this is a bare one.
    pub(super) fn bare_param(self) -> Option<u16> {
        match self {
            LTy::Param {
                index,
                optional: false,
            } => Some(index),
            _ => None,
        }
    }

    pub(super) fn bare_scalar_type(self) -> Option<ScalarType> {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => Some(scalar),
            _ => None,
        }
    }

    pub(super) fn spelling(self, records: &TypeRegistry) -> String {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Nominal { id, optional } => (records.nominal(id).name.clone(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
            LTy::Struct { ty, optional } => (
                records
                    .inst_spelling(TypeInstId::Record(ty))
                    .or_else(|| records.struct_by_type(ty).map(|info| info.name.clone()))
                    .unwrap_or_else(|| "struct".to_string()),
                optional,
            ),
            LTy::Enum { ty, optional } => {
                let base = records
                    .inst_spelling(TypeInstId::Enum(ty))
                    .or_else(|| records.enum_by_id(ty).map(|info| info.name.clone()))
                    .unwrap_or_else(|| "enum".to_string());
                (base, optional)
            }
            LTy::Collection { idx, optional } => (records.collection_spelling(idx), optional),
            LTy::Param { index, optional } => (format!("type parameter #{index}"), optional),
            // A program declares one store root, so the identity spelling needs no root
            // discriminator to stay unambiguous in a diagnostic.
            LTy::Identity { optional, .. } => ("Id(^root)".to_string(), optional),
        };
        if optional { format!("{base}?") } else { base }
    }

    pub(super) fn spelling_in(
        self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
    ) -> Result<String, LowerInvariant> {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Nominal { id, optional } => (records.nominal(id).name.clone(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
            LTy::Struct { ty, optional } => (metadata.garg_spelling(GArg::Struct(ty))?, optional),
            LTy::Enum { ty, optional } => (metadata.garg_spelling(GArg::Enum(ty))?, optional),
            LTy::Collection { idx, optional } => {
                (metadata.garg_spelling(GArg::Collection(idx))?, optional)
            }
            LTy::Param { index, optional } => (format!("type parameter #{index}"), optional),
            LTy::Identity { optional, .. } => ("Id(^root)".to_string(), optional),
        };
        Ok(if optional { format!("{base}?") } else { base })
    }

    /// The bare nominal identity, if this is one.
    pub(super) fn bare_nominal(self) -> Option<NominalId> {
        match self {
            LTy::Nominal {
                id,
                optional: false,
            } => Some(id),
            _ => None,
        }
    }

    /// The bare enum identity, if this is one.
    pub(super) fn bare_enum(self) -> Option<EnumId> {
        match self {
            LTy::Enum {
                ty,
                optional: false,
            } => Some(ty),
            _ => None,
        }
    }

    /// The bare entry-identity root, if this is one.
    pub(super) fn bare_identity(self) -> Option<u16> {
        match self {
            LTy::Identity {
                root,
                optional: false,
            } => Some(root),
            _ => None,
        }
    }

    /// This type as a built-in generic argument (a bare value type), or `None` for
    /// an optional or the durable resource record, which are not value arguments.
    pub(super) fn as_garg(self) -> Option<GArg> {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => Some(GArg::Scalar(scalar)),
            LTy::Nominal {
                id,
                optional: false,
            } => Some(GArg::Nominal(id)),
            LTy::Struct {
                ty,
                optional: false,
            } => Some(GArg::Struct(ty)),
            LTy::Enum {
                ty,
                optional: false,
            } => Some(GArg::Enum(ty)),
            LTy::Collection {
                idx,
                optional: false,
            } => Some(GArg::Collection(idx)),
            LTy::Param {
                index,
                optional: false,
            } => Some(GArg::Param(index)),
            _ => None,
        }
    }

    pub(super) fn image(self) -> ImageType {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => ImageType::scalar(scalar.image()),
            LTy::Scalar {
                scalar,
                optional: true,
            } => ImageType::opt_scalar(scalar.image()),
            // A nominal is int-shaped in the image; its interval is enforced by
            // the emitted range guards, not by the recorded type.
            LTy::Nominal {
                optional: false, ..
            } => ImageType::scalar(Scalar::Int),
            LTy::Nominal { optional: true, .. } => ImageType::opt_scalar(Scalar::Int),
            LTy::Record { ty, optional } | LTy::Struct { ty, optional } => ImageType::Record {
                idx: ty.index(),
                optional,
            },
            LTy::Enum { ty, optional } => ImageType::Enum {
                idx: ty.index(),
                optional,
            },
            LTy::Collection { idx, optional } => ImageType::Collection { idx, optional },
            // Only reached in the discarded template-check draft; the sentinel keeps
            // that throwaway image well-formed and is never encoded.
            LTy::Param {
                optional: false, ..
            } => ImageType::scalar(Scalar::Int),
            LTy::Param { optional: true, .. } => ImageType::opt_scalar(Scalar::Int),
            LTy::Identity { root, optional } => ImageType::Identity { root, optional },
        }
    }
}
