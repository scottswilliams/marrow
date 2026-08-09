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
//! `0600`. No raw descriptor or `rustix` type escapes the public API, and this
//! crate contains no `unsafe` code.
//!
//! `ENOSYS`, `ENOTSUP`, `EOPNOTSUPP`, unsupported `EINVAL`, `EXDEV`, identity
//! drift, unsupported semantics, and an unqualified platform all fail closed as
//! typed refusals. The qualified platforms are Darwin (the `rustix` libc
//! backend) and Linux on `x86_64`/`aarch64` (the `rustix` `linux_raw` backend);
//! every operation on any other platform returns a typed
//! unqualified-platform refusal.
//!
//! Portable identity checks are not a kernel compare-and-swap. The safety claim
//! requires an exclusive or private admitted parent plus the cooperative
//! [`CacheLock`]; malicious same-UID mutation outside that custody remains an
//! explicit limitation.
//!
//! # Durability envelope
//!
//! The established claim is atomic publication plus process- and OS-crash
//! recovery inside the documented file-and-directory-`fsync` envelope. Every
//! sync in this crate is a plain `fsync`; `fcntl_fullfsync` is not used because
//! no current envelope claims power-loss durability. Sudden-power-loss or
//! drive-cache-reset durability on macOS is not established.

mod custody;
mod entry;
mod frame;
mod journal;
mod lock;
mod sys;

pub use custody::{AdmittedDir, CustodyError, EntryStat, FsIdentity, NodeKind, OpenedFile};
pub use entry::{EntryName, EntryNameError};
pub use frame::{
    DecodedFrame, FrameCorruption, FrameLawError, JournalCommon, JournalKind, PhaseRecord,
    TailState, decode_frame, encode_header, encode_record,
};
pub use journal::{
    ClaimedJournal, CorruptionReason, JournalError, JournalWitness, LiveJournal, PendingJournal,
    PendingName, PendingState, PreclaimDebris, RetainedCorruption, classify, claim,
};
pub use lock::{CacheLock, LockError};
