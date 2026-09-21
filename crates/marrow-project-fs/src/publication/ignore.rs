//! The version-control ignore entry that keeps this project's publication
//! transients untracked.
//!
//! Every removal the publication protocol performs rests on these names never
//! being tracked: a committed transient is recreated by every checkout, and a
//! checkout writing one while a publication is running can lose it. An entry
//! that cannot be shown to keep the names untracked therefore refuses the
//! acquisition with [`super::IdsRefusal::UntrackedContract`].

use marrow_fs_journal::{AdmittedDir, CustodyError};
use marrow_project::IDS_ENTRY;

use super::{
    IdsPublicationError, IdsRefusal, LOCK_NAME, PendingName, admitted_name, quarantine_spelling,
    stage_spelling,
};

/// The version-control ignore entry the write owner keeps beside the entries
/// it names.
pub(super) const IGNORE_NAME: &str = ".gitignore";
/// The comment the written ignore block carries above the entry names.
const IGNORE_COMMENT: &str = "\
# Machine-written by Marrow. The cooperative write lock is machine-local runtime
# state, and the other entries are a publication in flight or the debris an
# interrupted one left. No checkout carries any of them; only `ids` is committed.
";
/// The opening every comment this owner has written begins with, and the whole
/// of what tells an entry this owner wrote from a developer's own file.
///
/// The comment's remaining words describe the name set they stand above and
/// change when that set does; this prefix does not, so it stays the mark.
const IGNORE_COMMENT_MARK: &str = "# Machine-written by Marrow.";
/// How much of an existing ignore entry is read to decide whether it already
/// names every entry this owner keeps untracked. A file this owner wrote is
/// eight lines — three of comment and five names; anything past this bound
/// belongs to whoever wrote it and is left exactly as found.
const IGNORE_READ_CEILING: usize = 4096;

/// Keep every entry no checkout may carry out of version control from the owner
/// that creates them, so a project carries no hand-written ignore line and a
/// fresh clone is correct without one.
///
/// The entry is completed rather than rewritten: a name is appended only when
/// the file does not already carry it, so a second acquisition writes nothing,
/// whatever a developer added survives, and the empty file a crash between the
/// create and the fill leaves is finished by the next acquisition. Completion
/// happens under the comment the entry already carries, so no entry ends up
/// with two. It runs under the write lock, so two first publications cannot
/// both append.
///
/// Four states refuse the acquisition with [`IdsRefusal::UntrackedContract`]:
/// an entry that cannot be read, one past the read bound, one missing names
/// that cannot be written, and one carrying a negation line. An entry that
/// already names every transient is left exactly as found, whatever its mode.
pub(super) fn install_untracked_ignore(meta: &AdmittedDir) -> Result<(), IdsPublicationError> {
    let name = admitted_name(IGNORE_NAME);
    let (created, found) = match meta.create_file_excl(&name) {
        Ok(created) => (Some(created), Vec::new()),
        Err(CustodyError::AlreadyExists { .. }) => {
            // Whether the entry is already complete is a read-only question, so
            // it is asked read-only: a checkout may carry the entry unwritable,
            // and an open that demanded write to decide it would refuse every
            // publication and recovery of a project that needs no append. A mode
            // withholding even that read is refused, because proceeding would
            // publish transients a later `git add -A` offers and a later
            // checkout writes.
            match meta.open_file_readonly(&name) {
                Ok(opened) => (None, opened.read_prefix(IGNORE_READ_CEILING + 1)?),
                Err(error) if access_withheld(&error) => {
                    return Err(IdsPublicationError::bare(IdsRefusal::UntrackedContract));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    };
    // A file larger than the read bound has not been read to the point that
    // decides the question, so whether it names the transients is unknown —
    // and unknown is refused as unreadable is. The bound also catches an entry
    // this owner's own appends pushed past it: past the bound no acquisition can
    // see the names it already wrote, so appending would repeat them forever.
    if found.len() > IGNORE_READ_CEILING {
        return Err(IdsPublicationError::bare(IdsRefusal::UntrackedContract));
    }
    // This owner writes no negation, so a `!` line is a hand edit. Whatever it
    // names, appending a name again would not change what Git re-includes — it
    // takes the last match — and this owner will not rewrite a line a developer
    // wrote, so the contract stays unestablished.
    if ignore_carries_negation(&found) {
        return Err(IdsPublicationError::bare(IdsRefusal::UntrackedContract));
    }
    let untracked = untracked_entry_names();
    let missing: Vec<String> = untracked
        .into_iter()
        .filter(|entry| !ignore_names_entry(&found, entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut block = String::new();
    if found.last().is_some_and(|byte| *byte != b'\n') {
        block.push('\n');
    }
    // An entry that already carries this owner's comment gains only the names; a
    // second copy would leave the first standing over a stale name set.
    if !ignore_carries_comment(&found) {
        block.push_str(IGNORE_COMMENT);
    }
    for entry in missing {
        block.push_str(&entry);
        block.push('\n');
    }
    let mut entry = match created {
        Some(created) => created,
        // Names are missing, so the entry does not yet keep this project's
        // transients untracked and must be appended to. An entry this process
        // may not write cannot be completed, so the acquisition refuses.
        None => match meta.open_file(&name) {
            Ok(opened) => opened,
            Err(error) if access_withheld(&error) => {
                return Err(IdsPublicationError::bare(IdsRefusal::UntrackedContract));
            }
            Err(error) => return Err(error.into()),
        },
    };
    entry.append(block.as_bytes())?;
    entry.sync()?;
    meta.sync()?;
    Ok(())
}

/// Whether a refused open says this process may not reach the entry's bytes,
/// rather than that the entry is not one this owner can maintain at all. Both of
/// the ignore entry's opens classify through here.
///
/// The custody owner reads a permission refusal over a regular file whose owner
/// bits fall short as [`CustodyError::ModeDenied`]; one it could not attribute
/// to those bits — another user's entry, a restrictive security policy — arrives
/// unclassified and is the same withheld access. An environmental write failure
/// is not in this family: it carries its own error kind and stays a refusal.
fn access_withheld(error: &CustodyError) -> bool {
    match error {
        CustodyError::ModeDenied { .. } => true,
        CustodyError::Io { source, .. } => source.kind() == std::io::ErrorKind::PermissionDenied,
        _ => false,
    }
}

/// Every `.marrow` entry this protocol can leave that no checkout may carry:
/// the machine-local write lock, the successor stage, the cleanup quarantine,
/// and the journal owner's two marker names. Each is derived from the same
/// constant the protocol mutates through, so a renamed or added transient
/// reaches the ignore entry with it rather than through a second hand-kept
/// list.
fn untracked_entry_names() -> Vec<String> {
    let ledger = admitted_name(IDS_ENTRY);
    let journal =
        PendingName::derive(&ledger).expect("the fixed journal names are admitted spellings");
    vec![
        LOCK_NAME.to_owned(),
        stage_spelling().to_owned(),
        quarantine_spelling().to_owned(),
        journal.pending().as_str().to_owned(),
        journal.claim().as_str().to_owned(),
    ]
}

/// The bytes read from the ignore entry as lines, each without the trailing
/// carriage return a CRLF checkout leaves.
fn ignore_lines(found: &[u8]) -> impl Iterator<Item = &[u8]> {
    found
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// Whether the bytes read from the ignore entry already name `entry`.
///
/// A line matches without the optional leading `/` that anchors a pattern to the
/// ignore file's own directory, as well as without the carriage return: every
/// such spelling names what this owner would append, and an entry shared with a
/// developer must not accumulate semantic duplicates. The form this owner writes
/// stays the bare name.
fn ignore_names_entry(found: &[u8], entry: &str) -> bool {
    ignore_lines(found).any(|line| line.strip_prefix(b"/").unwrap_or(line) == entry.as_bytes())
}

/// Whether the ignore entry carries any negation line. This owner writes none,
/// so one is a developer's edit that may re-include a transient by name or by
/// pattern; gitignore pattern syntax is not modelled to tell which.
fn ignore_carries_negation(found: &[u8]) -> bool {
    ignore_lines(found).any(|line| line.starts_with(b"!"))
}

/// Whether the bytes read from the ignore entry already carry this owner's
/// comment, in any wording it has been written with.
fn ignore_carries_comment(found: &[u8]) -> bool {
    ignore_lines(found).any(|line| line.starts_with(IGNORE_COMMENT_MARK.as_bytes()))
}
