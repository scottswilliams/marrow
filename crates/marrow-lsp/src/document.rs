//! The bounded open-document ledger, its per-document budget, and the coordinator's
//! checked monotonic revision counter.
//!
//! The ledger is keyed by [`DocumentKey`] and holds [`DocumentState::OpenText`] or
//! [`DocumentState::OpenUnavailable`]. Full-document sync only: a `didChange` carries
//! one whole-document replacement, never a range. Each newly opened key reserves one
//! [`MAX_OPEN_DOCUMENTS`]-bounded budget slot that pre-accounts the record plus its
//! maximum bounded failure evidence, so a later text→unavailable replacement cannot
//! grow unaccounted retention. Every accepted transition consumes the successor of the
//! coordinator's one [`RevisionCounter`]; an invalid notification consumes no revision.

use std::collections::HashMap;

use marrow_compile::InputRevision;

use crate::capacities::MAX_OPEN_DOCUMENTS;
use crate::uri::DocumentKey;

/// The coordinator's single checked monotonic revision source. Every accepted
/// open/change/close obtains its successor; exhaustion is a fixed terminal.
pub struct RevisionCounter {
    /// The next revision to hand out, or `None` once the maximum has been issued.
    next: Option<u64>,
}

impl RevisionCounter {
    /// The counter installed at initialization, seeded to a fixed initial revision.
    pub fn initial() -> (Self, InputRevision) {
        // The fixed initial revision is 0; the first successor an accepted transition
        // takes is 1.
        (Self { next: Some(1) }, InputRevision::new(0))
    }

    /// The next revision, or [`RevisionExhausted`] once every value has been issued.
    pub fn advance(&mut self) -> Result<InputRevision, RevisionExhausted> {
        let value = self.next.ok_or(RevisionExhausted)?;
        // The successor is `None` once the maximum value has been handed out, so the
        // maximum is usable and the following call fails closed.
        self.next = value.checked_add(1);
        Ok(InputRevision::new(value))
    }
}

/// The checked revision counter would overflow. A fixed terminal: the server fail-stops
/// before reuse, wrap, or saturation.
#[derive(Debug, PartialEq, Eq)]
pub struct RevisionExhausted;

/// Bounded evidence for why an open document is unavailable, already rendered through
/// the capture facade's operational writer (never re-rendered by the ledger).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableEvidence {
    /// The stable marrow diagnostic code string.
    pub code: &'static str,
    /// The bounded operational message.
    pub message: String,
}

/// The state of one open document. Full-document sync: the text is the whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentState {
    /// The document is open with an admitted whole-document body at a version.
    OpenText {
        /// The client-assigned version.
        version: i32,
        /// The whole-document text.
        text: String,
    },
    /// The document is open but its last transition was refused by overlay admission.
    OpenUnavailable {
        /// The client-assigned version.
        version: i32,
        /// Bounded failure evidence.
        failure: UnavailableEvidence,
    },
}

impl DocumentState {
    /// The version of this state.
    pub fn version(&self) -> i32 {
        match self {
            DocumentState::OpenText { version, .. }
            | DocumentState::OpenUnavailable { version, .. } => *version,
        }
    }

    /// Whether this state carries admitted text.
    pub fn is_text(&self) -> bool {
        matches!(self, DocumentState::OpenText { .. })
    }
}

/// Why a document notification was refused at the ledger.
#[derive(Debug, PartialEq, Eq)]
pub enum LedgerRefusal {
    /// The ledger is full; a fixed terminal.
    Exhausted,
    /// The notification is malformed against ledger state (duplicate open, unknown
    /// change/close, equal/decreasing version). Discarded with no mutation.
    Discard,
}

/// The bounded open-document ledger.
pub struct DocumentLedger {
    entries: HashMap<DocumentKey, DocumentState>,
}

impl DocumentLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The number of open documents.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The state of an open document.
    pub fn get(&self, key: &DocumentKey) -> Option<&DocumentState> {
        self.entries.get(key)
    }

    /// Whether every open entry currently carries text (no `OpenUnavailable`). An empty
    /// ledger is trivially all-text.
    pub fn all_available(&self) -> bool {
        self.entries.values().all(DocumentState::is_text)
    }

    /// Iterate the open text entries as `(relative key, text)` for overlay construction.
    pub fn text_entries(&self) -> impl Iterator<Item = (&DocumentKey, &str)> {
        self.entries.iter().filter_map(|(key, state)| match state {
            DocumentState::OpenText { text, .. } => Some((key, text.as_str())),
            DocumentState::OpenUnavailable { .. } => None,
        })
    }

    /// Validate a `didOpen`: the key must not already be open, and a fresh key must fit
    /// the bounded ledger. On success the coordinator advances the revision and installs
    /// the new state.
    pub fn validate_open(&self, key: &DocumentKey) -> Result<(), LedgerRefusal> {
        if self.entries.contains_key(key) {
            // A duplicate open is discarded with no mutation.
            return Err(LedgerRefusal::Discard);
        }
        if self.entries.len() >= MAX_OPEN_DOCUMENTS {
            return Err(LedgerRefusal::Exhausted);
        }
        Ok(())
    }

    /// Install an open state after a successful `validate_open` and revision advance.
    pub fn insert(&mut self, key: DocumentKey, state: DocumentState) {
        self.entries.insert(key, state);
    }

    /// Validate a `didChange`: the key must be open and the new version strictly
    /// greater than the current one.
    pub fn validate_change(&self, key: &DocumentKey, version: i32) -> Result<(), LedgerRefusal> {
        match self.entries.get(key) {
            Some(state) if version > state.version() => Ok(()),
            // Unknown key, or an equal/decreasing version: discard.
            _ => Err(LedgerRefusal::Discard),
        }
    }

    /// Replace an open entry's state after a successful `validate_change` and revision
    /// advance.
    pub fn replace(&mut self, key: &DocumentKey, state: DocumentState) {
        if let Some(slot) = self.entries.get_mut(key) {
            *slot = state;
        }
    }

    /// Validate a `didClose`: the key must be open.
    pub fn validate_close(&self, key: &DocumentKey) -> Result<(), LedgerRefusal> {
        if self.entries.contains_key(key) {
            Ok(())
        } else {
            Err(LedgerRefusal::Discard)
        }
    }

    /// Remove a closed entry after a successful `validate_close` and revision advance.
    pub fn remove(&mut self, key: &DocumentKey) {
        self.entries.remove(key);
    }
}

impl Default for DocumentLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uri::SelectedRoot;

    fn key(name: &str) -> DocumentKey {
        let root = SelectedRoot::from_uri("file:///proj").unwrap();
        DocumentKey::from_uri(&format!("file:///proj/{name}"), &root).unwrap()
    }

    fn text(version: i32, body: &str) -> DocumentState {
        DocumentState::OpenText {
            version,
            text: body.to_owned(),
        }
    }

    #[test]
    fn revision_counter_starts_at_fixed_initial_and_advances() {
        let (mut counter, initial) = RevisionCounter::initial();
        assert_eq!(initial.get(), 0);
        assert_eq!(counter.advance().unwrap().get(), 1);
        assert_eq!(counter.advance().unwrap().get(), 2);
    }

    #[test]
    fn revision_counter_reports_exhaustion() {
        let mut counter = RevisionCounter {
            next: Some(u64::MAX),
        };
        assert_eq!(counter.advance().unwrap().get(), u64::MAX);
        assert_eq!(counter.advance(), Err(RevisionExhausted));
    }

    #[test]
    fn open_then_change_then_close() {
        let mut ledger = DocumentLedger::new();
        let k = key("src/a.mw");
        ledger.validate_open(&k).unwrap();
        ledger.insert(k.clone(), text(1, "a"));
        assert_eq!(ledger.len(), 1);

        assert_eq!(ledger.validate_change(&k, 1), Err(LedgerRefusal::Discard));
        ledger.validate_change(&k, 2).unwrap();
        ledger.replace(&k, text(2, "aa"));

        ledger.validate_close(&k).unwrap();
        ledger.remove(&k);
        assert!(ledger.is_empty());
    }

    #[test]
    fn duplicate_open_is_discarded() {
        let mut ledger = DocumentLedger::new();
        let k = key("src/a.mw");
        ledger.validate_open(&k).unwrap();
        ledger.insert(k.clone(), text(1, "a"));
        assert_eq!(ledger.validate_open(&k), Err(LedgerRefusal::Discard));
    }

    #[test]
    fn unknown_change_and_close_are_discarded() {
        let ledger = DocumentLedger::new();
        let k = key("src/a.mw");
        assert_eq!(ledger.validate_change(&k, 5), Err(LedgerRefusal::Discard));
        assert_eq!(ledger.validate_close(&k), Err(LedgerRefusal::Discard));
    }

    #[test]
    fn all_available_reflects_unavailable_entries() {
        let mut ledger = DocumentLedger::new();
        let k = key("src/a.mw");
        ledger.validate_open(&k).unwrap();
        ledger.insert(
            k.clone(),
            DocumentState::OpenUnavailable {
                version: 1,
                failure: UnavailableEvidence {
                    code: "project.source_path",
                    message: "x".to_owned(),
                },
            },
        );
        assert!(!ledger.all_available());
    }
}
