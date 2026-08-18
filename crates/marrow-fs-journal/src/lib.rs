//! The sole descriptor-rooted filesystem publication owner.
//!
//! This crate owns descriptor-relative path admission and mutation, cooperative
//! lock custody, the bounded five-kind pending-journal frame, journal replay,
//! sync, and crash-debris classification. Identity, lineage, lifecycle, and
//! package-cache publication rows consume it; none implements a second
//! rename/link/sync/recovery model.
//!
//! # Safe boundary
//!
//! The typed boundary is [`EntryName`], [`AdmittedDir`], [`OpenedFile`], and
//! [`CacheLock`]. [`EntryName`] admits one normal relative component and
//! rejects empty, `.`, `..`, separator, NUL, absolute/prefix/root, and
//! platform-invalid spelling before any filesystem call. Directories are
//! admitted from a retained trusted directory descriptor with
//! `DIRECTORY | NOFOLLOW | CLOEXEC`; file creation is `CREATE | EXCL` with mode
//! `0600`. No raw descriptor and no type of the private syscall adapter's
//! dependency escapes the public API, and this crate contains no `unsafe`
//! code.
//!
//! `ENOSYS`, `ENOTSUP`, `EOPNOTSUPP`, unsupported `EINVAL`, `EXDEV`, identity
//! drift, unsupported semantics, and an unqualified platform all fail closed as
//! typed refusals. The qualified platforms are Darwin (the adapter's libc
//! backend) and Linux on `x86_64`/`aarch64` (the adapter's `linux_raw`
//! backend); every operation on any other platform returns a typed
//! unqualified-platform refusal.
//!
//! Portable identity checks are not a kernel compare-and-swap. The safety claim
//! requires an exclusive or private admitted parent plus the cooperative
//! [`CacheLock`]; malicious same-UID mutation outside that custody remains an
//! explicit limitation.
//!
//! One crash window stays open by design. Entry creation is an `openat` whose
//! mode the process umask masks, followed by an `fchmod` that restores the
//! exact documented mode; the creating open is granted whatever mode it lands,
//! so only a crash between the two leaves an entry carrying the masked mode.
//! Closing the window would mean creating every entry under a generated
//! temporary name and linking it into place, which trades a rare wrong-mode
//! entry for debris under names this crate — which enumerates no directory —
//! could never reach again.
//!
//! A umask that strips owner read or write (`0477` or `0277`, say) therefore
//! leaves an entry that no later open of this crate reaches from a process
//! those bits bind: preclaim debris it cannot read, or a lock entry it cannot
//! reacquire. Both refuse with [`CustodyError::ModeDenied`], which names the
//! entry's observed mode and the mode to restore. Restoring that mode returns
//! the entry to ordinary classification; this crate performs no path-based
//! `chmod` of its own, because repairing a name it has not opened would write
//! through whatever that name maps to at that instant.
//!
//! A process holding the mode-override capability (`root`, or
//! `CAP_DAC_OVERRIDE` on Linux) is bound by none of those bits. Its opens
//! succeed, so it sees no such refusal, and lock acquisition restores the
//! documented `0600` on the entry itself.
//!
//! # Durability envelope
//!
//! The established claim is atomic publication plus process- and OS-crash
//! recovery inside the documented file-and-directory-`fsync` envelope. Every
//! sync in this crate is a plain `fsync`; the Darwin full-flush fcntl is not
//! used because no current envelope claims power-loss durability, and the
//! conformance suite keeps both facts conspicuous. Sudden-power-loss or
//! drive-cache-reset durability on macOS is not established.

mod custody;
mod entry;
mod frame;
mod journal;
mod lock;
mod sys;

pub use custody::{
    AdmittedDir, CustodyError, EntryStat, FsIdentity, NodeKind, OpenedFile, qualified_platform,
};
pub use entry::{EntryName, EntryNameError};
pub use frame::{
    DecodedFrame, FrameCorruption, FrameLawError, JournalCommon, JournalKind, PhaseRecord,
    TailState, decode_frame, encode_header, encode_record,
};
pub use journal::{
    BuiltHeader, ClaimRefusal, ClaimedJournal, CorruptionReason, JournalError, JournalWitness,
    LiveJournal, MarkerNames, MarkerShape, MarkerStats, PendingJournal, PendingName, PendingState,
    PreclaimDebris, claim, classify,
};
pub use lock::{CacheLock, LockError};
