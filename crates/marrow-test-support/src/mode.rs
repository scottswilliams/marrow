//! Unix mode-bit probes for the suites that plant a withheld permission.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::Scratch;

/// Require that permission bits actually deny this process the access a mode
/// withholds.
///
/// A check that planted a stripped mode nothing enforces would assert a refusal
/// that never happened, so this panics rather than reporting green. Mode bits do
/// not bind a process holding the mode-override capability (`root`, or
/// `CAP_DAC_OVERRIDE` on Linux), and a filesystem that carries no mode bits does
/// not enforce them at all.
pub fn require_mode_bits_bind(scratch: &Scratch) {
    let probe = scratch.path().join("deny-probe");
    std::fs::write(&probe, b"").expect("plant the probe");
    set_mode(&probe, 0o000);
    let denied = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&probe)
        .is_err();
    set_mode(&probe, 0o600);
    std::fs::remove_file(&probe).expect("remove the probe");
    assert!(
        denied,
        "mode 0000 under {} did not refuse a read-write open, so this check never ran. Run \
         the suite as a process the mode bits bind, on a filesystem that carries them.",
        scratch.path().display()
    );
}

pub fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
}

pub fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("stat entry").mode() & 0o7777
}
