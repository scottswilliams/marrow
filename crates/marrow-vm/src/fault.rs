//! Typed source-mapped runtime faults (design failure family 3).
//!
//! A fault carries a stable `run.*` code and the source line/column of the
//! faulting instruction. Runtime faults are source-uncatchable: they are distinct
//! from the language `Result<T,E>`, from source diagnostics, and from artifact
//! rejections.

use std::rc::Rc;

/// A runtime fault, mapped to the source position of the faulting instruction.
/// `detail` carries the static author text of an `unreachable("...")` fault; every
/// other fault has none. It is presentation-only: the stable typed fault surface is
/// the code and span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFault {
    code: &'static str,
    line: u32,
    column: u32,
    detail: Option<Rc<str>>,
}

impl RuntimeFault {
    pub(crate) fn new(code: &'static str, line: u32, column: u32) -> Self {
        Self {
            code,
            line,
            column,
            detail: None,
        }
    }

    /// A fault carrying static author text (an `unreachable("...")` invariant).
    pub(crate) fn with_detail(code: &'static str, line: u32, column: u32, detail: Rc<str>) -> Self {
        Self {
            code,
            line,
            column,
            detail: Some(detail),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    pub fn column(&self) -> u32 {
        self.column
    }

    /// The static author text, present only for an `unreachable("...")` fault.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl std::fmt::Display for RuntimeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}:{}", self.code, self.line, self.column)
    }
}

impl std::error::Error for RuntimeFault {}
