//! The abstract value type the phase-3 stack interpreter tracks.

use marrow_image::{ImageType, Scalar};

use crate::sealed::RetShape;

/// A verified operand-stack slot type. Optionals are tracked distinctly from bare
/// values, so a `T?` can never reach a bare-`T` consumer on any path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VType {
    Scalar { scalar: Scalar, optional: bool },
    Record { idx: u16, optional: bool },
    Enum { idx: u16, optional: bool },
    Collection { idx: u16, optional: bool },
    /// An entry identity `Id(^root)`, by ROOTS-table index. Tracked distinctly so an
    /// identity of one root never satisfies a consumer expecting another root or a
    /// key scalar.
    Identity { root: u16, optional: bool },
}

impl VType {
    pub(crate) fn bare_scalar(scalar: Scalar) -> Self {
        VType::Scalar {
            scalar,
            optional: false,
        }
    }

    pub(crate) fn bare_record(idx: u16) -> Self {
        VType::Record {
            idx,
            optional: false,
        }
    }

    pub(crate) fn bare_enum(idx: u16) -> Self {
        VType::Enum {
            idx,
            optional: false,
        }
    }

    pub(crate) fn bare_collection(idx: u16) -> Self {
        VType::Collection {
            idx,
            optional: false,
        }
    }

    /// The stack type for an image type reference (a parameter type), or `None` for
    /// `Unit`, which is never a parameter or local type. Records carry their sealed
    /// type index; the verifier proved it in range at decode.
    pub(crate) fn from_image(ty: ImageType) -> Option<Self> {
        match ty {
            ImageType::Unit => None,
            ImageType::Scalar { scalar, optional } => Some(VType::Scalar { scalar, optional }),
            ImageType::Record { idx, optional } => Some(VType::Record { idx, optional }),
            ImageType::Enum { idx, optional } => Some(VType::Enum { idx, optional }),
            ImageType::Collection { idx, optional } => Some(VType::Collection { idx, optional }),
            ImageType::Identity { root, optional } => Some(VType::Identity { root, optional }),
        }
    }

    pub(crate) fn is_optional(self) -> bool {
        match self {
            VType::Scalar { optional, .. }
            | VType::Record { optional, .. }
            | VType::Enum { optional, .. }
            | VType::Collection { optional, .. }
            | VType::Identity { optional, .. } => optional,
        }
    }

    /// The bare (non-optional) form of this type.
    pub(crate) fn to_bare(self) -> Self {
        match self {
            VType::Scalar { scalar, .. } => VType::Scalar {
                scalar,
                optional: false,
            },
            VType::Record { idx, .. } => VType::Record {
                idx,
                optional: false,
            },
            VType::Enum { idx, .. } => VType::Enum {
                idx,
                optional: false,
            },
            VType::Collection { idx, .. } => VType::Collection {
                idx,
                optional: false,
            },
            VType::Identity { root, .. } => VType::Identity {
                root,
                optional: false,
            },
        }
    }

    /// The optional form of this bare type.
    pub(crate) fn to_optional(self) -> Self {
        match self {
            VType::Scalar { scalar, .. } => VType::Scalar {
                scalar,
                optional: true,
            },
            VType::Record { idx, .. } => VType::Record {
                idx,
                optional: true,
            },
            VType::Enum { idx, .. } => VType::Enum {
                idx,
                optional: true,
            },
            VType::Collection { idx, .. } => VType::Collection {
                idx,
                optional: true,
            },
            VType::Identity { root, .. } => VType::Identity {
                root,
                optional: true,
            },
        }
    }

    /// Whether this stack type satisfies a function's declared return shape.
    pub(crate) fn matches_ret(self, ret: RetShape) -> bool {
        match (self, ret) {
            (
                VType::Scalar { scalar, optional },
                RetShape::Scalar {
                    scalar: want,
                    optional: want_opt,
                },
            ) => scalar == want && optional == want_opt,
            (
                VType::Record { idx, optional },
                RetShape::Record {
                    idx: want,
                    optional: want_opt,
                },
            ) => idx == want && optional == want_opt,
            (
                VType::Enum { idx, optional },
                RetShape::Enum {
                    idx: want,
                    optional: want_opt,
                },
            ) => idx == want && optional == want_opt,
            (
                VType::Collection { idx, optional },
                RetShape::Collection {
                    idx: want,
                    optional: want_opt,
                },
            ) => idx == want && optional == want_opt,
            (
                VType::Identity { root, optional },
                RetShape::Identity {
                    root: want,
                    optional: want_opt,
                },
            ) => root == want && optional == want_opt,
            _ => false,
        }
    }
}
