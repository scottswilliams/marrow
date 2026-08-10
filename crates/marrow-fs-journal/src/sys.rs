//! The single platform adapter and the crate's only `rustix` consumer.
//!
//! Qualification is a compile-time platform gate with a fail-closed stub:
//! Darwin uses the rustix libc backend, Linux on `x86_64`/`aarch64` the
//! `linux_raw` backend, and every other platform gets an adapter whose
//! handles are uninhabited and whose one constructor returns a typed
//! unqualified-platform refusal. Errno classification happens here once; no
//! rustix type leaves this module.

#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
mod imp {
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::FileExt;
    use std::path::Path;

    use rustix::fs::{AtFlags, FlockOperation, Mode, OFlags, RenameFlags, Stat};
    use rustix::io::Errno;

    use crate::custody::{CustodyError, EntryStat, FsIdentity, NodeKind};

    pub(crate) type DirHandle = OwnedFd;
    pub(crate) type FileHandle = File;

    /// How an errno should be read for one operation: whether the final
    /// component was opened `NOFOLLOW` (so `ELOOP` means a refused symlink)
    /// and whether `EINVAL` means the flag semantics are unsupported (the
    /// flagged rename operations).
    #[derive(Clone, Copy)]
    enum Reading {
        Plain,
        Nofollow,
        RenameFlagged,
    }

    /// The one errno classifier. `ENOSYS`, `ENOTSUP`, `EOPNOTSUPP`,
    /// unsupported `EINVAL`, and `EXDEV` fail closed as typed refusals; the
    /// comparisons are an `if` chain because two of the constants coincide on
    /// Linux.
    fn map(op: &'static str, reading: Reading, errno: Errno) -> CustodyError {
        if errno == Errno::NOSYS
            || errno == Errno::NOTSUP
            || errno == Errno::OPNOTSUPP
            || errno == Errno::XDEV
        {
            return CustodyError::Unsupported { op };
        }
        if matches!(reading, Reading::RenameFlagged) && errno == Errno::INVAL {
            return CustodyError::Unsupported { op };
        }
        if errno == Errno::EXIST {
            return CustodyError::AlreadyExists { op };
        }
        if errno == Errno::NOENT {
            return CustodyError::NotFound { op };
        }
        if matches!(reading, Reading::Nofollow) && errno == Errno::LOOP {
            return CustodyError::SymlinkRefused { op };
        }
        if errno == Errno::NOTDIR {
            return CustodyError::NotADirectory { op };
        }
        if errno == Errno::ISDIR {
            return CustodyError::WrongNodeKind {
                op,
                found: NodeKind::Directory,
            };
        }
        CustodyError::Io {
            op,
            source: std::io::Error::from(errno),
        }
    }

    fn dir_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }

    /// Journal file mode: `0600` exactly.
    fn file_mode() -> Mode {
        Mode::RUSR | Mode::WUSR
    }

    /// Created directory mode: `0700` exactly.
    fn dir_mode() -> Mode {
        Mode::RWXU
    }

    /// The lossless per-platform `st_dev`/`st_ino` projection: an injective
    /// widening on Darwin (`i32` bit-cast then zero-extended) and the native
    /// `u64` pair on Linux. No truncation exists on either qualified platform.
    #[cfg(target_os = "macos")]
    fn dev_ino(stat: &Stat) -> (u64, u64) {
        (u64::from(stat.st_dev.cast_unsigned()), stat.st_ino)
    }

    #[cfg(target_os = "linux")]
    fn dev_ino(stat: &Stat) -> (u64, u64) {
        (stat.st_dev, stat.st_ino)
    }

    fn project(stat: &Stat) -> EntryStat {
        let kind = match rustix::fs::FileType::from_raw_mode(stat.st_mode) {
            rustix::fs::FileType::Directory => NodeKind::Directory,
            rustix::fs::FileType::RegularFile => NodeKind::Regular,
            rustix::fs::FileType::Symlink => NodeKind::Symlink,
            _ => NodeKind::Other,
        };
        let (dev, ino) = dev_ino(stat);
        EntryStat {
            identity: FsIdentity::new(dev, ino),
            kind,
            nlink: u64::from(stat.st_nlink),
            size: stat.st_size.cast_unsigned(),
            mode: u32::from(stat.st_mode) & 0o7777,
        }
    }

    pub(crate) fn open_dir_root(path: &Path) -> Result<DirHandle, CustodyError> {
        rustix::fs::open(path, dir_flags(), Mode::empty())
            .map_err(|errno| map("admit directory", Reading::Nofollow, errno))
    }

    pub(crate) fn open_dir_child(dir: &DirHandle, name: &str) -> Result<DirHandle, CustodyError> {
        rustix::fs::openat(dir, name, dir_flags(), Mode::empty())
            .map_err(|errno| map("admit directory", Reading::Nofollow, errno))
    }

    pub(crate) fn mkdir_child(dir: &DirHandle, name: &str) -> Result<(), CustodyError> {
        rustix::fs::mkdirat(dir, name, dir_mode())
            .map_err(|errno| map("create directory", Reading::Plain, errno))
    }

    /// `mkdirat`'s requested mode is masked by the process umask; the
    /// documented exact 0700 is restored on the admitted descriptor.
    pub(crate) fn restore_dir_mode(dir: &DirHandle) -> Result<(), CustodyError> {
        rustix::fs::fchmod(dir, dir_mode())
            .map_err(|errno| map("create directory", Reading::Plain, errno))
    }

    pub(crate) fn create_file_excl(
        dir: &DirHandle,
        name: &str,
    ) -> Result<FileHandle, CustodyError> {
        let flags = OFlags::RDWR
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | OFlags::APPEND;
        let file = rustix::fs::openat(dir, name, flags, file_mode())
            .map(File::from)
            .map_err(|errno| map("create file", Reading::Nofollow, errno))?;
        // The open-time mode is masked by the process umask; the exact 0600
        // the claim law rechecks is restored on the creating descriptor
        // before any use, so no umask can manufacture a wrong-mode claim.
        if let Err(errno) = rustix::fs::fchmod(&file, file_mode()) {
            remove_created(dir, name, &file);
            return Err(map("create file", Reading::Plain, errno));
        }
        Ok(file)
    }

    /// Best-effort removal of an entry this call created and is about to
    /// refuse, so a refused creation returns no entry the caller was never
    /// handed. The unlink is issued only after a stat witnesses that the name
    /// still maps to the created inode: nothing is removed on the evidence of
    /// the name alone. The witness and the unlink are separate calls, so a
    /// replacement landing between them is still removed — the same
    /// stat-then-unlink window the witnessed discard carries, and the reason
    /// the safety claim needs an exclusive or private admitted parent. The
    /// removal is synced like every other entry mutation of this crate. If the
    /// witnessing stat, the unlink, or the sync fails, the entry survives as
    /// never-linked debris, which the pending-journal classification reads as
    /// preclaim.
    fn remove_created(dir: &DirHandle, name: &str, file: &FileHandle) {
        let (Ok(created), Ok(Some(present))) = (fstat_file(file), stat_entry(dir, name)) else {
            return;
        };
        if present.identity == created.identity
            && rustix::fs::unlinkat(dir, name, AtFlags::empty()).is_ok()
        {
            let _ = sync_dir(dir);
        }
    }

    pub(crate) fn open_file(dir: &DirHandle, name: &str) -> Result<FileHandle, CustodyError> {
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::APPEND;
        rustix::fs::openat(dir, name, flags, Mode::empty())
            .map(File::from)
            .map_err(|errno| map("open file", Reading::Nofollow, errno))
    }

    /// Open a file for reading alone: witness-and-inspect custody over debris
    /// whose mode may deny write access must not require more than read.
    pub(crate) fn open_file_readonly(
        dir: &DirHandle,
        name: &str,
    ) -> Result<FileHandle, CustodyError> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        rustix::fs::openat(dir, name, flags, Mode::empty())
            .map(File::from)
            .map_err(|errno| map("open file", Reading::Nofollow, errno))
    }

    pub(crate) fn open_lock_file(dir: &DirHandle, name: &str) -> Result<FileHandle, CustodyError> {
        let flags = OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        rustix::fs::openat(dir, name, flags, file_mode())
            .map(File::from)
            .map_err(|errno| map("open lock", Reading::Nofollow, errno))
    }

    /// Restore the lock entry's exact `0600`. A just-created entry carries the
    /// umask-masked mode, and a wrong-mode entry would refuse the next
    /// same-user reopen; the call is idempotent for an existing entry. The
    /// caller admits the node as a regular file first, so a planted
    /// non-regular node is refused before any mode is written and no refusal
    /// that a non-regular node can reach stands between the open and this
    /// restore.
    pub(crate) fn restore_lock_mode(file: &FileHandle) -> Result<(), CustodyError> {
        rustix::fs::fchmod(file, file_mode())
            .map_err(|errno| map("open lock", Reading::Plain, errno))
    }

    /// `flock(LOCK_EX | LOCK_NB)`. `Ok(false)` reports a held lock.
    pub(crate) fn try_lock_exclusive(file: &FileHandle) -> Result<bool, CustodyError> {
        match rustix::fs::flock(file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(true),
            Err(errno) if errno == Errno::WOULDBLOCK || errno == Errno::AGAIN => Ok(false),
            Err(errno) => Err(map("lock", Reading::Plain, errno)),
        }
    }

    pub(crate) fn link(dir: &DirHandle, existing: &str, new: &str) -> Result<(), CustodyError> {
        rustix::fs::linkat(dir, existing, dir, new, AtFlags::empty())
            .map_err(|errno| map("link", Reading::Plain, errno))
    }

    pub(crate) fn unlink(dir: &DirHandle, name: &str) -> Result<(), CustodyError> {
        rustix::fs::unlinkat(dir, name, AtFlags::empty())
            .map_err(|errno| map("unlink", Reading::Plain, errno))
    }

    pub(crate) fn stat_entry(
        dir: &DirHandle,
        name: &str,
    ) -> Result<Option<EntryStat>, CustodyError> {
        match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(project(&stat))),
            Err(errno) if errno == Errno::NOENT => Ok(None),
            Err(errno) => Err(map("stat entry", Reading::Plain, errno)),
        }
    }

    /// A no-follow stat of the one path-based custody entry (the trusted
    /// root), used only to refine an already-issued admission refusal.
    pub(crate) fn lstat_path(path: &Path) -> Result<Option<EntryStat>, CustodyError> {
        match rustix::fs::lstat(path) {
            Ok(stat) => Ok(Some(project(&stat))),
            Err(errno) if errno == Errno::NOENT => Ok(None),
            Err(errno) => Err(map("stat path", Reading::Plain, errno)),
        }
    }

    pub(crate) fn fstat_dir(dir: &DirHandle) -> Result<EntryStat, CustodyError> {
        rustix::fs::fstat(dir)
            .map(|stat| project(&stat))
            .map_err(|errno| map("stat directory", Reading::Plain, errno))
    }

    pub(crate) fn fstat_file(file: &FileHandle) -> Result<EntryStat, CustodyError> {
        rustix::fs::fstat(file)
            .map(|stat| project(&stat))
            .map_err(|errno| map("stat file", Reading::Plain, errno))
    }

    /// Plain `fsync` on the directory: the documented durability envelope.
    pub(crate) fn sync_dir(dir: &DirHandle) -> Result<(), CustodyError> {
        rustix::fs::fsync(dir).map_err(|errno| map("sync directory", Reading::Plain, errno))
    }

    /// Plain `fsync` on the file: the documented durability envelope.
    pub(crate) fn sync_file(file: &FileHandle) -> Result<(), CustodyError> {
        rustix::fs::fsync(file).map_err(|errno| map("sync file", Reading::Plain, errno))
    }

    pub(crate) fn truncate_file(file: &FileHandle, len: u64) -> Result<(), CustodyError> {
        rustix::fs::ftruncate(file, len).map_err(|errno| map("truncate", Reading::Plain, errno))
    }

    /// `renameat` with `EXCHANGE` (`renameatx_np(RENAME_SWAP)` on Darwin,
    /// `renameat2(RENAME_EXCHANGE)` on Linux). `ENOSYS` and unsupported
    /// `EINVAL` are typed unsupported-platform refusals, never a fallback.
    pub(crate) fn exchange(dir: &DirHandle, first: &str, second: &str) -> Result<(), CustodyError> {
        rustix::fs::renameat_with(dir, first, dir, second, RenameFlags::EXCHANGE)
            .map_err(|errno| map("exchange", Reading::RenameFlagged, errno))
    }

    /// `renameat` with `NOREPLACE` (`renameatx_np(RENAME_EXCL)` on Darwin,
    /// `renameat2(RENAME_NOREPLACE)` on Linux). Same fail-closed reading.
    pub(crate) fn rename_noreplace(
        dir: &DirHandle,
        from: &str,
        to: &str,
    ) -> Result<(), CustodyError> {
        rustix::fs::renameat_with(dir, from, dir, to, RenameFlags::NOREPLACE)
            .map_err(|errno| map("rename-noreplace", Reading::RenameFlagged, errno))
    }

    pub(crate) fn append(file: &mut FileHandle, bytes: &[u8]) -> Result<(), CustodyError> {
        file.write_all(bytes).map_err(|source| CustodyError::Io {
            op: "append",
            source,
        })
    }

    /// Read at most `max` bytes from offset zero through the retained handle.
    /// The allocation is guided by a same-handle `fstat` — one byte past the
    /// statted size, so a file that grew after the stat fills the buffer and
    /// forces the `max`-sized retry instead of silently returning a short
    /// prefix — and the caller's kind ceiling, never an untrusted length
    /// field, still bounds the total bytes read.
    pub(crate) fn read_prefix(file: &FileHandle, max: usize) -> Result<Vec<u8>, CustodyError> {
        let statted = fstat_file(file)?.size.saturating_add(1);
        let hint = usize::try_from(statted).unwrap_or(max).min(max);
        let mut buffer = vec![0u8; hint];
        let mut filled = 0usize;
        while filled < max {
            if filled == buffer.len() {
                buffer.resize(max, 0);
            }
            match file.read_at(&mut buffer[filled..], filled as u64) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(source) => return Err(CustodyError::Io { op: "read", source }),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every retained descriptor is close-on-exec, and journal files are
        /// append-mode: the flags are read back through `fcntl` rather than
        /// trusted from the open call.
        #[test]
        fn retained_descriptors_are_cloexec_and_journal_files_append() {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let root = std::env::temp_dir().join(format!(
                "marrow-fs-journal-sys-flags-{}-{nonce}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&root).expect("create scratch");

            let dir = open_dir_root(&root).expect("open scratch root");
            let dir_fd_flags = rustix::io::fcntl_getfd(&dir).expect("dir descriptor flags");
            assert!(dir_fd_flags.contains(rustix::io::FdFlags::CLOEXEC));

            let file = create_file_excl(&dir, "flags-probe").expect("create probe");
            let file_fd_flags = rustix::io::fcntl_getfd(&file).expect("file descriptor flags");
            assert!(file_fd_flags.contains(rustix::io::FdFlags::CLOEXEC));
            let status = rustix::fs::fcntl_getfl(&file).expect("file status flags");
            assert!(status.contains(OFlags::APPEND));

            let lock = open_lock_file(&dir, "lock-probe").expect("open lock probe");
            let lock_fd_flags = rustix::io::fcntl_getfd(&lock).expect("lock descriptor flags");
            assert!(lock_fd_flags.contains(rustix::io::FdFlags::CLOEXEC));
            assert!(
                !rustix::fs::fcntl_getfl(&lock)
                    .expect("lock status flags")
                    .contains(OFlags::APPEND),
                "the lock entry is not a journal; it carries no append mode",
            );

            drop(lock);
            drop(file);
            drop(dir);
            let _ = std::fs::remove_dir_all(&root);
        }

        /// The typed refusal classification, pinned per errno.
        #[test]
        fn errno_reading_fails_closed() {
            assert!(matches!(
                map("op", Reading::Plain, Errno::NOSYS),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::NOTSUP),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::OPNOTSUPP),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::XDEV),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::RenameFlagged, Errno::INVAL),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::RenameFlagged, Errno::NOSYS),
                CustodyError::Unsupported { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::INVAL),
                CustodyError::Io { .. }
            ));
            assert!(matches!(
                map("op", Reading::Nofollow, Errno::LOOP),
                CustodyError::SymlinkRefused { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::LOOP),
                CustodyError::Io { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::EXIST),
                CustodyError::AlreadyExists { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::NOENT),
                CustodyError::NotFound { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::NOTDIR),
                CustodyError::NotADirectory { .. }
            ));
            assert!(matches!(
                map("op", Reading::Plain, Errno::ISDIR),
                CustodyError::WrongNodeKind {
                    found: NodeKind::Directory,
                    ..
                }
            ));
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod imp {
    //! The fail-closed stub: handles are uninhabited, so no operation on an
    //! unqualified platform can be represented, and the one constructor
    //! refuses with the typed unqualified-platform error.

    use std::path::Path;

    use crate::custody::{CustodyError, EntryStat};

    pub(crate) enum DirHandle {}
    pub(crate) enum FileHandle {}

    fn refuse() -> CustodyError {
        CustodyError::UnqualifiedPlatform {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    pub(crate) fn open_dir_root(_path: &Path) -> Result<DirHandle, CustodyError> {
        Err(refuse())
    }

    pub(crate) fn open_dir_child(dir: &DirHandle, _name: &str) -> Result<DirHandle, CustodyError> {
        match *dir {}
    }

    pub(crate) fn mkdir_child(dir: &DirHandle, _name: &str) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn restore_dir_mode(dir: &DirHandle) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn create_file_excl(
        dir: &DirHandle,
        _name: &str,
    ) -> Result<FileHandle, CustodyError> {
        match *dir {}
    }

    pub(crate) fn open_file(dir: &DirHandle, _name: &str) -> Result<FileHandle, CustodyError> {
        match *dir {}
    }

    pub(crate) fn open_file_readonly(
        dir: &DirHandle,
        _name: &str,
    ) -> Result<FileHandle, CustodyError> {
        match *dir {}
    }

    pub(crate) fn open_lock_file(dir: &DirHandle, _name: &str) -> Result<FileHandle, CustodyError> {
        match *dir {}
    }

    pub(crate) fn restore_lock_mode(file: &FileHandle) -> Result<(), CustodyError> {
        match *file {}
    }

    pub(crate) fn try_lock_exclusive(file: &FileHandle) -> Result<bool, CustodyError> {
        match *file {}
    }

    pub(crate) fn link(dir: &DirHandle, _existing: &str, _new: &str) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn unlink(dir: &DirHandle, _name: &str) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn stat_entry(
        dir: &DirHandle,
        _name: &str,
    ) -> Result<Option<EntryStat>, CustodyError> {
        match *dir {}
    }

    pub(crate) fn lstat_path(_path: &Path) -> Result<Option<EntryStat>, CustodyError> {
        Err(refuse())
    }

    pub(crate) fn fstat_dir(dir: &DirHandle) -> Result<EntryStat, CustodyError> {
        match *dir {}
    }

    pub(crate) fn fstat_file(file: &FileHandle) -> Result<EntryStat, CustodyError> {
        match *file {}
    }

    pub(crate) fn sync_dir(dir: &DirHandle) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn sync_file(file: &FileHandle) -> Result<(), CustodyError> {
        match *file {}
    }

    pub(crate) fn truncate_file(file: &FileHandle, _len: u64) -> Result<(), CustodyError> {
        match *file {}
    }

    pub(crate) fn exchange(
        dir: &DirHandle,
        _first: &str,
        _second: &str,
    ) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn rename_noreplace(
        dir: &DirHandle,
        _from: &str,
        _to: &str,
    ) -> Result<(), CustodyError> {
        match *dir {}
    }

    pub(crate) fn append(file: &mut FileHandle, _bytes: &[u8]) -> Result<(), CustodyError> {
        match *file {}
    }

    pub(crate) fn read_prefix(file: &FileHandle, _max: usize) -> Result<Vec<u8>, CustodyError> {
        match *file {}
    }
}

pub(crate) use imp::*;
