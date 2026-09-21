//! The sole descriptor-rooted filesystem publication owner: entry-name
//! admission, admitted-directory custody, the cooperative cache lock, and the
//! bounded pending-journal frame with replay and crash-debris classification.
//!
//! The typed boundary is [`EntryName`], [`AdmittedDir`], [`OpenedFile`], and
//! [`CacheLock`]; no raw descriptor or adapter type escapes it, and there is no
//! `unsafe` code. Journal files are created `CREATE | EXCL`, and every entry
//! this crate creates has its exact mode restored by `fchmod` on the creating
//! descriptor, so a crash between the two calls leaves a umask-masked mode
//! that later opens refuse with [`CustodyError::ModeDenied`]; the crate
//! performs no path-based `chmod`.
//!
//! The qualified platforms are Darwin and Linux on `x86_64`/`aarch64`; any
//! other platform, unsupported semantic, or identity drift is a typed refusal.
//! The safety claim needs an exclusive or private admitted parent plus the
//! cooperative [`CacheLock`], and every sync is a plain `fsync`: atomic
//! publication and crash recovery are established, power-loss durability is not.

mod custody;
mod entry;
mod frame;
mod journal;
mod lock;
mod sys;

pub use custody::{
    AdmittedDir, CustodyError, CustodyOp, EntryStat, FsIdentity, NodeKind, OpenedFile,
    qualified_platform,
};
pub use entry::{EntryName, EntryNameError};
pub use frame::{
    DecodedFrame, FrameCorruption, FrameLawError, JournalCommon, PhaseRecord, RecordLaw, TailState,
    encode_header, encode_record,
};
pub use journal::{
    BuiltHeader, ClaimRefusal, ClaimedJournal, CorruptionReason, JournalError, JournalWitness,
    LiveJournal, MarkerNames, MarkerShape, MarkerStats, PendingJournal, PendingName, PendingState,
    PreclaimDebris, claim, classify,
};
pub use lock::{CacheLock, LockError};
