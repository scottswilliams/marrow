use std::io::{self, Write};
use std::process::ExitCode;

/// A usage refusal: the specific problem, then where to find usage. Exit 2 keeps a
/// mistyped invocation from passing a CI gate green. Written without a panicking
/// print macro, so a closed diagnostic channel refuses rather than aborts.
pub(crate) fn usage(message: &str) -> ExitCode {
    let _ = writeln!(
        io::stderr().lock(),
        "{message}; run marrow --help for usage"
    );
    ExitCode::from(2)
}

/// Delivery can fail after execution completed. Report it without replaying the
/// command, even when its diagnostic channel is also unavailable.
pub(crate) fn finish(result: io::Result<ExitCode>) -> ExitCode {
    match result {
        Ok(exit) => exit,
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "{}: {error}",
                marrow_codes::Code::IoWrite.as_str()
            );
            ExitCode::FAILURE
        }
    }
}
