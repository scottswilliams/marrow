use std::io::{self, Write};
use std::process::ExitCode;

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
