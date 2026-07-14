//! The abstract value type the phase-3 stack interpreter tracks.

use marrow_image::Scalar;

use crate::sealed::RetShape;

/// A verified operand-stack slot type. Optionals are tracked distinctly from bare
/// values, so a `T?` can never reach a bare-`T` consumer on any path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VType {
    Scalar { scalar: Scalar, optional: bool },
    Record { idx: u16, optional: bool },
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

    pub(crate) fn is_optional(self) -> bool {
        match self {
            VType::Scalar { optional, .. } | VType::Record { optional, .. } => optional,
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
            _ => false,
        }
    }
}
