//! The lowered value type ([`LTy`]) and its conversions to image and generic-argument forms.

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
    /// A dense `struct` value: image-`Record`- and runtime-`Value::Record`-shaped like
    /// [`LTy::Record`], but a distinct value type — constructible, returnable, every
    /// field present. The `TypeId` names its image record def.
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
        idx: CollTypeId,
        optional: bool,
    },
    /// An abstract generic type parameter, present only while the once-checked
    /// template pass lowers a generic body against a throwaway draft. `index` is the
    /// parameter's declaration position; its constraint is read from the lowerer's
    /// type environment. A monomorphized instantiation never carries a `Param`.
    Param {
        index: TypeParamIndex,
        optional: bool,
    },
    /// An entry identity `Id(^root)`, image-`Identity`- and runtime-`Value::Id`-shaped.
    /// `root` is the store root's ROOTS-table index (0 — a program has one root). A
    /// distinct value type: a by-value runtime/lookup value, not a durable field or key.
    Identity {
        root: RootId,
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

    pub(super) fn bare_param(self) -> Option<TypeParamIndex> {
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

    /// This type's source spelling, with composite instance spellings read from the
    /// registry's already-minted set. A composite the registry has not named yet falls
    /// back to its declaration name, then to its kind.
    pub(super) fn spelling(self, records: &TypeRegistry) -> String {
        self.spell(records, |composite| match composite {
            Composite::Struct(ty) => records
                .inst_spelling(TypeInstId::Record(ty))
                .or_else(|| records.struct_by_type(ty).map(|info| info.name.clone()))
                .unwrap_or_else(|| "struct".to_string()),
            Composite::Enum(ty) => records
                .inst_spelling(TypeInstId::Enum(ty))
                .or_else(|| records.enum_by_id(ty).map(|info| info.name.clone()))
                .unwrap_or_else(|| "enum".to_string()),
            Composite::Collection(idx) => records.collection_spelling(idx),
        })
    }

    /// This type's source spelling inside a live metadata session, which can mint the
    /// spelling of a composite instance the registry has not named yet. The one renderer
    /// below produces both forms, so a diagnostic and the image metadata cannot drift on
    /// a type's name.
    pub(super) fn spelling_in(
        self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
    ) -> Result<String, LowerInvariant> {
        let minted = match self.composite() {
            Some(composite) => metadata.garg_spelling(composite.garg())?,
            None => String::new(),
        };
        Ok(self.spell(records, move |_| minted))
    }

    /// The composite instance whose spelling this type defers to, if any.
    fn composite(self) -> Option<Composite> {
        match self {
            LTy::Struct { ty, .. } => Some(Composite::Struct(ty)),
            LTy::Enum { ty, .. } => Some(Composite::Enum(ty)),
            LTy::Collection { idx, .. } => Some(Composite::Collection(idx)),
            _ => None,
        }
    }

    /// The one spelling renderer. `composite` supplies the base spelling of a struct,
    /// enum, or collection instance; every other arm is spelled from the registry alone.
    fn spell(self, records: &TypeRegistry, composite: impl FnOnce(Composite) -> String) -> String {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Nominal { id, optional } => (records.nominal(id).name.clone(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
            LTy::Struct { ty, optional } => (composite(Composite::Struct(ty)), optional),
            LTy::Enum { ty, optional } => (composite(Composite::Enum(ty)), optional),
            LTy::Collection { idx, optional } => (composite(Composite::Collection(idx)), optional),
            LTy::Param { index, optional } => (format!("type parameter #{index}"), optional),
            // A program declares one store root, so the identity spelling needs no root
            // discriminator to stay unambiguous in a diagnostic.
            LTy::Identity { optional, .. } => ("Id(^root)".to_string(), optional),
        };
        if optional { format!("{base}?") } else { base }
    }

    pub(super) fn bare_nominal(self) -> Option<NominalId> {
        match self {
            LTy::Nominal {
                id,
                optional: false,
            } => Some(id),
            _ => None,
        }
    }

    pub(super) fn bare_enum(self) -> Option<EnumId> {
        match self {
            LTy::Enum {
                ty,
                optional: false,
            } => Some(ty),
            _ => None,
        }
    }

    pub(super) fn bare_identity(self) -> Option<RootId> {
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
            LTy::Record { ty, optional } | LTy::Struct { ty, optional } => {
                ImageType::Record { idx: ty, optional }
            }
            LTy::Enum { ty, optional } => ImageType::Enum { idx: ty, optional },
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

/// A composite type instance whose spelling is minted rather than declared.
#[derive(Clone, Copy)]
enum Composite {
    Struct(TypeId),
    Enum(EnumId),
    Collection(CollTypeId),
}

impl Composite {
    fn garg(self) -> GArg {
        match self {
            Composite::Struct(ty) => GArg::Struct(ty),
            Composite::Enum(ty) => GArg::Enum(ty),
            Composite::Collection(idx) => GArg::Collection(idx),
        }
    }
}
