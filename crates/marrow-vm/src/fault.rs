//! Typed source-mapped runtime faults (design failure family 3).
//!
//! A fault carries a stable `run.*` code and the source line/column of the
//! faulting instruction. Runtime faults are source-uncatchable: they are distinct
//! from the language `Result<T,E>`, from source diagnostics, and from artifact
//! rejections.

/// A runtime fault, mapped to the source position of the faulting instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFault {
    code: &'static str,
    line: u32,
    column: u32,
}

impl RuntimeFault {
    pub(crate) fn new(code: &'static str, line: u32, column: u32) -> Self {
        Self { code, line, column }
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
}

impl std::fmt::Display for RuntimeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}:{}", self.code, self.line, self.column)
    }
}

impl std::error::Error for RuntimeFault {}
