//! The declaration coordinate tables the declare pass owns: where each declared
//! `struct` and `resource` was written.
//!
//! Those are the families a later pass reports at. The value-containment cycle check
//! reports each struct and record on a cycle at its own declaration; a cyclic generic
//! instantiation is reported at its template's own span instead, and a declared enum's
//! payload is a bare scalar, so a non-generic payload cannot name an enum. So
//! `declare_enums` records no coordinate, and an enum has no row here.
//!
//! A pass that must report at a declaration reads the coordinate from here instead of
//! scanning the syntax tree for a declaration whose name matches. That scan was linear
//! in the project's declarations and ran once per reported type, and it could only be
//! written at all because the pass still borrowed the tree.
//!
//! The tables are owned fields of [`super::TypeRegistry`], the bundle declaration
//! admission builds and hands out whole. Declaration admission is one-shot: a pass
//! that fails returns `Err` and the partially built registry is dropped with
//! everything it owns, so a failed admission leaves no coordinate reachable and
//! there is no window in which a stale row could be read. That ownership is the
//! whole rollback discipline for these tables. **If declaration admission ever
//! becomes transactional, they owe the same inverse the generic owners already
//! carry in `super::owner_txn`**, because a surviving prefix would then outlive
//! the declarations that minted it.

use std::collections::BTreeMap;
use std::ops::Deref;

use marrow_image::TypeId;
use marrow_project::FileIdentity;
use marrow_syntax::SourceSpan;

use super::RecordInfo;
use crate::analysis::FileRef;

/// The admitted `resource` records, each with the position of the declaration it was
/// built from in the resource slice the declare pass was given.
///
/// Index `i` of one addresses index `i` of the other, and the whole mutable surface is
/// what keeps that true rather than the private fields alone. [`Self::admit`] is the
/// only append and it appends to both vectors, so a helper that pushed a record alone
/// would leave every later record carrying another declaration's ordinal; [`Self::at_mut`]
/// is the only other mutation, and it edits one record where it lies. No route hands out
/// a `&mut [RecordInfo]`, so `swap`, `sort`, `reverse`, `truncate` and slice assignment —
/// each of which moves a record out from under a fixed ordinal — do not compile against
/// this type. The durable build reads the pairing rather than rebuilding one from
/// resource name spellings. Reading is the record slice itself; the ordinals are read
/// through [`Self::ordinals`].
#[derive(Default)]
pub(crate) struct AdmittedRecords {
    records: Vec<RecordInfo>,
    declarations: Vec<usize>,
}

impl AdmittedRecords {
    /// Admit `record`, built from the declaration at `ordinal`.
    pub(super) fn admit(&mut self, record: RecordInfo, ordinal: usize) {
        self.records.push(record);
        self.declarations.push(ordinal);
    }

    /// For each admitted record, in record order, its declaration's position.
    pub(super) fn ordinals(&self) -> &[usize] {
        &self.declarations
    }

    /// The record at `index`, to fill in place. The reserve-then-fill pass edits a
    /// record where it lies, which is the only mutation this type admits besides
    /// [`Self::admit`]: a mutable slice would additionally let a caller reorder or
    /// replace records while the ordinals stayed put.
    pub(super) fn at_mut(&mut self, index: usize) -> &mut RecordInfo {
        &mut self.records[index]
    }
}

impl Deref for AdmittedRecords {
    type Target = [RecordInfo];

    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

/// Where one declared value type was written: its module and its name span.
///
/// The module is the existing [`FileRef`] coordinate rather than a second module
/// ordinal invented here — the compiler already has one owner for "which module",
/// and a coordinate table is not a reason to mint another. The span is held
/// inline: this is the only row family citing it today, so a shared span table
/// would be indirection without sharing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DeclarationCoordinate {
    at: FileRef,
    span: SourceSpan,
}

/// The declare pass's coordinate tables.
///
/// Deliberately not `Clone` and never returned by value: a copy handed to a caller
/// would outlive the admission that minted it, which is exactly the stale-row
/// window the one-shot ownership above rules out.
#[derive(Default)]
pub(crate) struct DeclarationCoordinates {
    /// One owned identity per module that declared a value type, not one per
    /// declaration. Keyed rather than appended because the declare pass is not
    /// required to finish one module's declarations before starting the next.
    files: BTreeMap<FileRef, FileIdentity>,
    declarations: BTreeMap<TypeId, DeclarationCoordinate>,
}

impl DeclarationCoordinates {
    /// Record where `type_id` was declared.
    ///
    /// A repeat for the same type would be the declare pass reserving one image
    /// type twice, which it does not do; the first coordinate stands, so a caller
    /// reporting at a declaration can never be steered to a later homonym.
    pub(super) fn declare(
        &mut self,
        type_id: TypeId,
        at: FileRef,
        file: &FileIdentity,
        span: SourceSpan,
    ) {
        self.files.entry(at).or_insert_with(|| file.clone());
        self.declarations
            .entry(type_id)
            .or_insert(DeclarationCoordinate { at, span });
    }

    /// The module position and span `type_id` was declared at, or `None` when this pass
    /// minted no coordinate for it.
    ///
    /// Distinct from [`resolve`](Self::resolve), which answers with the module's SPELLING
    /// for a diagnostic to print. This answers with its position, which is what a consumer
    /// checking that two references denote one declaration needs: a spelling can repeat
    /// across two parses of a project, a position within one admitted project cannot.
    pub(super) fn module_of(&self, type_id: TypeId) -> Option<(FileRef, SourceSpan)> {
        let coordinate = self.declarations.get(&type_id)?;
        Some((coordinate.at, coordinate.span))
    }

    /// Where `type_id` was declared, or `None` for a type this pass minted no
    /// coordinate for — a reserved toolchain template has no source declaration.
    pub(super) fn resolve(&self, type_id: TypeId) -> Option<(&FileIdentity, SourceSpan)> {
        let coordinate = self.declarations.get(&type_id)?;
        let file = self.files.get(&coordinate.at)?;
        Some((file, coordinate.span))
    }
}
