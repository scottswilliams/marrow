//! A standard-output handle that refuses every write, for the suites that drive a
//! companion whose output stream has gone away.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::Stdio;

/// Shutdown survives descriptor duplication by concurrent child launches.
pub fn broken_output() -> Stdio {
    let (writer, peer) = UnixStream::pair().expect("output socket pair");
    writer.shutdown(Shutdown::Write).expect("disable output");
    let mut output = File::from(OwnedFd::from(writer));
    assert_eq!(
        output
            .write(b"x")
            .expect_err("output must reject writes")
            .kind(),
        ErrorKind::BrokenPipe
    );
    drop(peer);
    output.into()
}
