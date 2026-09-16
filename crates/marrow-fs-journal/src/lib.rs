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
//! documented mode, so only a crash between the two leaves an entry carrying
//! the masked mode. Creating under a generated temporary name and linking into
//! place would close it, at the cost of debris under names this crate — which
//! enumerates no directory — could never reach again.
//!
//! A umask that strips owner read or write leaves such an entry unreachable to
//! any later open bound by those bits: preclaim debris it cannot read, or a
//! lock entry it cannot reacquire. Both refuse with
//! [`CustodyError::ModeDenied`], naming the observed mode and the mode to
//! restore; restoring it returns the entry to ordinary classification. This
//! crate performs no path-based `chmod` of its own, because repairing a name it
//! has not opened would write through whatever that name maps to at that
//! instant. A process holding the mode-override capability (`root`, or
//! `CAP_DAC_OVERRIDE` on Linux) is bound by none of those bits.
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
    AdmittedDir, CustodyError, CustodyOp, EntryStat, FsIdentity, LockAcquisition, NodeKind,
    OpenedFile, qualified_platform,
};
pub use entry::{EntryName, EntryNameError};
pub use frame::{
    DecodedFrame, FrameCorruption, FrameLawError, JournalCommon, JournalKind, PhaseRecord,
    RecordLaw, TailState, decode_frame, encode_header, encode_record,
};
pub use journal::{
    BuiltHeader, ClaimRefusal, ClaimedJournal, CorruptionReason, JournalError, JournalWitness,
    LiveJournal, MarkerNames, MarkerShape, MarkerStats, PendingJournal, PendingName, PendingState,
    PreclaimDebris, claim, classify,
};
pub use lock::{CacheLock, LockError};
