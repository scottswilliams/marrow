//! External boundary test: a true consumer that imports only `marrow_project_fs`.
//!
//! It names the diagnostic-code registry, the four pure-owner reexports, and the
//! opaque capture failure, and exhaustively matches the two closed support enums a
//! thin consumer observes. The paired `compile_fail` doctests in the crate root
//! prove the opaque family and the sealed pure errors cannot be constructed or
//! matched from here.

use marrow_project_fs::{
    CaptureError, CaptureFailure, CapturePresentation, Code, LinkPosition, ManifestError,
    OverlayBound, OverlayEntry, OverlayFailure, OverlayReason, OverlaySnapshot, PhysicalBound,
    PhysicalFailure, PhysicalIoError, PhysicalKind, PhysicalOperation, PhysicalRefusal,
    PhysicalRole, Position, ProjectInput,
};

fn name<T>() {}

fn match_physical_role(role: PhysicalRole) {
    match role {
        PhysicalRole::Root
        | PhysicalRole::Manifest
        | PhysicalRole::IdentityLedger
        | PhysicalRole::SourceRoot
        | PhysicalRole::SourceDirectory
        | PhysicalRole::SourceFile => {}
    }
}

fn match_physical_operation(operation: PhysicalOperation) {
    match operation {
        PhysicalOperation::Canonicalize
        | PhysicalOperation::Inspect
        | PhysicalOperation::Open
        | PhysicalOperation::Enumerate
        | PhysicalOperation::Retain
        | PhysicalOperation::Read
        | PhysicalOperation::Recheck => {}
    }
}

fn match_physical_refusal(refusal: PhysicalRefusal) {
    match refusal {
        PhysicalRefusal::Missing { error } | PhysicalRefusal::Io { error } => {
            observe_io(&error);
        }
        PhysicalRefusal::Link { position } => match position {
            LinkPosition::Terminal | LinkPosition::Intermediate => {}
        },
        PhysicalRefusal::UnexpectedKind { expected, actual } => {
            for kind in [expected, actual] {
                match kind {
                    PhysicalKind::RegularFile | PhysicalKind::Directory | PhysicalKind::Other => {}
                }
            }
        }
        PhysicalRefusal::Hardlink
        | PhysicalRefusal::InvalidPathEncoding
        | PhysicalRefusal::Changed
        | PhysicalRefusal::UnsupportedPlatform => {}
        PhysicalRefusal::Bound {
            bound,
            limit,
            actual,
        } => {
            match bound {
                PhysicalBound::ManifestBytes
                | PhysicalBound::IdentityLedgerBytes
                | PhysicalBound::VisitedEntries
                | PhysicalBound::TraversalDepth
                | PhysicalBound::SourceFiles
                | PhysicalBound::SourceFileBytes
                | PhysicalBound::SourceTotalBytes
                | PhysicalBound::RetainedPathUnits
                | PhysicalBound::PathWorkUnits => {}
            }
            let _ = (limit, actual);
        }
    }
}

fn observe_io(error: &PhysicalIoError) {
    // Only the typed kind and raw OS code are observable.
    let _ = (error.kind(), error.raw_os_error());
}

fn match_overlay_reason(reason: OverlayReason) {
    match reason {
        OverlayReason::Bound {
            bound,
            limit,
            actual,
            entry,
        } => {
            match bound {
                OverlayBound::Entries
                | OverlayBound::KeyBytes
                | OverlayBound::FileBytes
                | OverlayBound::TotalBytes => {}
            }
            let _ = (limit, actual, entry.map(|index| index.get()));
        }
        OverlayReason::Allocation { entry } => {
            let _ = entry.map(|index| index.get());
        }
        OverlayReason::Duplicate { first, second } => {
            let _ = (first.get(), second.get());
        }
        OverlayReason::Noncanonical { entry }
        | OverlayReason::Nonmember { entry }
        | OverlayReason::WrongRole { entry } => {
            let _ = entry.get();
        }
    }
}

#[test]
fn the_public_facade_is_named_and_exhaustively_matchable_from_outside() {
    // The diagnostic-code registry and the four pure-owner reexports are nameable
    // through this crate alone, with no direct `marrow-project` edge.
    name::<Code>();
    name::<ProjectInput>();
    name::<ManifestError>();
    name::<CaptureError>();
    name::<Position>();

    // The opaque capture failure and the transparent support types are nameable.
    name::<CaptureFailure>();
    name::<PhysicalFailure>();
    name::<PhysicalIoError>();
    name::<OverlayFailure>();
    name::<OverlaySnapshot<'static>>();
    name::<CapturePresentation<'static>>();

    // A borrowed overlay entry is constructible and stays borrowed.
    let entry = OverlayEntry::new("src/main.mw", b"fn main() {}");
    let _entry: OverlayEntry<'_> = entry;

    // The two closed support enums observable through the facade are exhaustively
    // matchable from outside the crate.
    let _: fn(PhysicalRole) = match_physical_role;
    let _: fn(PhysicalOperation) = match_physical_operation;
    let _: fn(PhysicalRefusal) = match_physical_refusal;
    let _: fn(OverlayReason) = match_overlay_reason;
}
