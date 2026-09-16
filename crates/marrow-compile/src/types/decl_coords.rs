//! The declaration coordinate tables the declare pass owns: where each declared
//! `struct` and `resource` was written, so a pass reporting at a declaration reads a
//! coordinate instead of scanning the syntax tree for a name match.
//!
//! Enums have no row: the value-containment cycle check is the only reader, a cyclic
//! generic instantiation is reported at its template's span, and a declared enum's
//! payload is a bare scalar, so no cycle passes through one.
//!
//! The tables are owned fields of [`super::TypeRegistry`]. Declaration admission is
//! one-shot — a pass that fails returns `Err` and the partially built registry is
//! dropped whole — so no stale coordinate is ever reachable.

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
/// Index `i` of one addresses index `i` of the other, and the type enforces that no
/// record can move: [`Self::admit`] is the only append and appends to both vectors,
/// and no route hands out a `&mut [RecordInfo]`, so `swap`, `sort`, `reverse`,
/// `truncate` and slice assignment do not compile against this type. Positions are
/// stable by construction; contents are not authenticated.
///
/// The durable build reads this pairing — the record slice and [`Self::ordinals`] —
/// rather than rebuilding one from resource name spellings.
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

    /// The record at `index`, for the reserve-then-fill pass to fill in place. Lending one
    /// record rather than a `&mut [RecordInfo]` keeps a caller from reordering records
    /// while the ordinals stay put; it still permits replacing the record at a fixed
    /// index, so the mutable surface is position-preserving, not authenticated.
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

/// Where one declared `struct` or `resource` was written: its module and name span.
///
/// The module is the existing [`FileRef`] coordinate, keeping one owner for "which
/// module". The span is inline because this is the only row family citing it, so a
/// shared span table would be indirection without sharing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DeclarationCoordinate {
    at: FileRef,
    span: SourceSpan,
}

/// The declare pass's coordinate tables.
///
/// Not `Clone` and never returned by value: a copy handed to a caller would outlive
/// the admission that minted it, reopening the stale-row window one-shot ownership
/// closes.
#[derive(Default)]
pub(crate) struct DeclarationCoordinates {
    /// One owned identity per module that declared a `struct` or `resource`, not one
    /// per declaration. Keyed rather than appended because the declare pass is not
    /// required to finish one module's declarations before starting the next.
    files: BTreeMap<FileRef, FileIdentity>,
    declarations: BTreeMap<TypeId, DeclarationCoordinate>,
}

impl DeclarationCoordinates {
    /// Record where `type_id` was declared. The first coordinate stands, so a caller
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
    /// Distinct from [`resolve`](Self::resolve), which answers with the module's spelling
    /// for a diagnostic to print. A position is unique within one admitted project where
    /// a spelling need not be, but every parse of a project repeats it: this locates a
    /// declaration, it does not authenticate one.
    pub(super) fn module_of(&self, type_id: TypeId) -> Option<(FileRef, SourceSpan)> {
        let coordinate = self.declarations.get(&type_id)?;
        Some((coordinate.at, coordinate.span))
    }

    /// Where `type_id` was declared, or `None` for a type this pass minted no coordinate
    /// for — an enum, or a reserved toolchain template with no source declaration.
    pub(super) fn resolve(&self, type_id: TypeId) -> Option<(&FileIdentity, SourceSpan)> {
        let coordinate = self.declarations.get(&type_id)?;
        let file = self.files.get(&coordinate.at)?;
        Some((file, coordinate.span))
    }
}
