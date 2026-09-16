//! The version-control ignore entry that keeps this project's publication
//! transients untracked, and the matching that decides whether it does.
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
/// that cannot be written, and one whose `!` line covers a name this owner
/// would otherwise have kept ignored. An entry that already names every
/// transient is left exactly as found, whatever its mode.
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
    // A negation re-including one of these names leaves it tracked whatever else
    // the entry says, and appending the name again would not change that: Git
    // takes the last match. This owner will not rewrite a line a developer
    // wrote, so the contract stays unestablished.
    let untracked = untracked_entry_names();
    if untracked
        .iter()
        .any(|entry| ignore_negates_entry(&found, entry))
    {
        return Err(IdsPublicationError::bare(IdsRefusal::UntrackedContract));
    }
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

/// Whether the ignore entry re-includes one of this owner's names with a
/// negation line.
///
/// A positive line naming a transient does not settle the question on its own:
/// a later `!` line covering the same name puts it back, and Git takes the last
/// match, so such an entry does not establish the contract.
///
/// A negation need not spell the name to cover it — `!*`, `!*.stage`, and
/// `!ids.publish.*` each re-include one. So the pattern is matched rather than
/// compared, over the gitignore syntax that can reach a name in this directory:
/// `*`, `?`, and `[...]`, with an optional leading or trailing `/`. A pattern
/// naming a path below this directory cannot reach these entries, which sit
/// directly in `.marrow`, so it does not refuse.
fn ignore_negates_entry(found: &[u8], entry: &str) -> bool {
    ignore_lines(found).any(|line| {
        let Some(pattern) = line.strip_prefix(b"!") else {
            return false;
        };
        // A trailing slash names a directory; these entries are not one, and
        // the slash is not part of the pattern either way.
        let pattern = pattern.strip_suffix(b"/").unwrap_or(pattern);
        // A leading slash anchors to the directory holding the ignore file,
        // which is where these entries are.
        let pattern = pattern.strip_prefix(b"/").unwrap_or(pattern);
        pattern_reaches(pattern, entry.as_bytes())
    })
}

/// Whether an ignore pattern can match an entry lying directly in the directory
/// its ignore file governs.
///
/// A `**` component matches zero or more directories, so it can vanish
/// entirely: `**/ids.publish.stage` names the entry sitting right here. Scope is
/// therefore decided by what remains once those components are dropped. One
/// component can still name a directly contained entry; two or more require a
/// subdirectory, and these entries never sit in one.
fn pattern_reaches(pattern: &[u8], name: &[u8]) -> bool {
    let mut components = pattern
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty() && *component != b"**".as_slice());
    let Some(only) = components.next() else {
        // Nothing but `**` components: the pattern reaches everything.
        return true;
    };
    if components.next().is_some() {
        return false;
    }
    wildcard_covers(only, name)
}

/// Whether one ignore-file path-component pattern matches `name` exactly.
///
/// The supported syntax is what can appear in a single component: `*` for any
/// run of characters, `?` for one, and `[...]` for a set, with `!` or `^`
/// negating the set and a backslash escaping the next character. A `[` that
/// never closes is a literal, as Git treats it.
///
/// # Which way this errs
///
/// A pattern wrongly read as reaching one of these names costs a refusal an
/// operator clears by editing one line; one wrongly read as reaching nothing
/// lets a transient stay tracked, which every removal bound in this protocol
/// assumes away. So every approximation is deliberately toward refusing: a POSIX
/// class inside a set matches any character rather than having its members read,
/// and a trailing `/**` — which in Git names a directory's contents rather than
/// the directory — is read as reaching the name.
///
/// Backtracking is bounded by construction: the only branch point is a `*`, and
/// the greedy retry walks forward through `name` without ever revisiting an
/// earlier star. Both inputs are bounded already — a name from this module's
/// fixed set, and a line from a file read under a 4 KiB ceiling.
fn wildcard_covers(pattern: &[u8], name: &[u8]) -> bool {
    let (mut pattern_at, mut name_at) = (0usize, 0usize);
    // Where to resume if the current `*` turns out to have matched too little.
    let (mut star_at, mut retry_at) = (None, 0usize);
    while name_at < name.len() {
        let advanced = match pattern.get(pattern_at) {
            Some(b'*') => {
                star_at = Some(pattern_at);
                pattern_at += 1;
                retry_at = name_at;
                continue;
            }
            Some(b'?') => {
                pattern_at += 1;
                name_at += 1;
                continue;
            }
            Some(b'[') => match match_set(&pattern[pattern_at..], name[name_at]) {
                Some((true, next)) => {
                    pattern_at += next;
                    name_at += 1;
                    continue;
                }
                Some((false, _)) => false,
                // An unclosed set is a literal `[`.
                None => name[name_at] == b'[',
            },
            Some(b'\\') => pattern.get(pattern_at + 1) == Some(&name[name_at]),
            Some(byte) => *byte == name[name_at],
            None => false,
        };
        if advanced {
            pattern_at += if pattern.get(pattern_at) == Some(&b'\\') {
                2
            } else {
                1
            };
            name_at += 1;
            continue;
        }
        // No match here: give the last `*` one more byte, or fail.
        let Some(star) = star_at else {
            return false;
        };
        pattern_at = star + 1;
        retry_at += 1;
        name_at = retry_at;
    }
    pattern[pattern_at..].iter().all(|byte| *byte == b'*')
}

/// Match one `[...]` set against `byte`, returning whether it matched and the
/// pattern offset just past the set. `None` when the set never closes, which
/// Git reads as a literal `[`.
///
/// A set carrying a POSIX class — `[[:alpha:]]` and its family — is answered as
/// matching, whatever the byte and whatever the negation, rather than having the
/// class's members read; see the over-approximation [`wildcard_covers`] states.
fn match_set(pattern: &[u8], byte: u8) -> Option<(bool, usize)> {
    let mut at = 1;
    let negated = matches!(pattern.get(at), Some(b'!' | b'^'));
    if negated {
        at += 1;
    }
    let mut matched = false;
    let mut carries_class = false;
    let mut first = true;
    while at < pattern.len() {
        if pattern[at] == b']' && !first {
            let verdict = if carries_class {
                true
            } else {
                matched != negated
            };
            return Some((verdict, at + 1));
        }
        first = false;
        if pattern[at] == b'[' && pattern.get(at + 1) == Some(&b':') {
            let mut scan = at + 2;
            loop {
                let (colon, close) = (pattern.get(scan)?, pattern.get(scan + 1)?);
                if *colon == b':' && *close == b']' {
                    break;
                }
                scan += 1;
            }
            carries_class = true;
            at = scan + 2;
            continue;
        }
        let low = if pattern[at] == b'\\' {
            at += 1;
            *pattern.get(at)?
        } else {
            pattern[at]
        };
        // A range, unless the `-` is the set's last character.
        if pattern.get(at + 1) == Some(&b'-') && pattern.get(at + 2).is_some_and(|end| *end != b']')
        {
            matched |= (low..=pattern[at + 2]).contains(&byte);
            at += 3;
        } else {
            matched |= low == byte;
            at += 1;
        }
    }
    None
}

/// Whether the bytes read from the ignore entry already carry this owner's
/// comment, in any wording it has been written with.
fn ignore_carries_comment(found: &[u8]) -> bool {
    ignore_lines(found).any(|line| line.starts_with(IGNORE_COMMENT_MARK.as_bytes()))
}
