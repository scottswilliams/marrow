//! The mode reading of a refused open, pinned per case.

use super::*;

fn permission_denied(op: CustodyOp) -> CustodyError {
    CustodyError::Io {
        op,
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    }
}

fn observed(kind: NodeKind, mode: u32) -> Option<EntryStat> {
    Some(EntryStat {
        identity: FsIdentity::new(1, 2),
        kind,
        nlink: 1,
        size: 0,
        mode,
    })
}

/// The mode reading of a refused open, pinned per case. It is what a crash
/// inside the create-then-`fchmod` window leaves behind on either
/// qualified platform, so it is asserted here without depending on the
/// running process's own override capability. The observed mode is the
/// whole of the evidence — no uid is read, on the entry or on this
/// process — so an entry another user owns whose owner bits fall short is
/// read the same way, and the refusal attributes the repair to the entry's
/// owner rather than to the caller.
#[test]
fn a_stripped_mode_is_read_as_the_typed_mode_refusal() {
    for (found, required) in [
        (0o400, 0o600),
        (0o200, 0o600),
        (0o000, 0o600),
        (0o200, 0o400),
    ] {
        let refined = refine_open_refusal(
            permission_denied(CustodyOp::OpenLock),
            observed(NodeKind::Regular, found),
            required,
        );
        assert!(
            matches!(refined, CustodyError::ModeDenied { op: CustodyOp::OpenLock, found: seen, required: needed }
                if seen == found && needed == required),
            "mode {found:o} against {required:o} was read as {refined:?}"
        );
    }
}

/// Every refusal the mode reading must leave alone: an entry that carries
/// the required bits was refused for some other reason, a non-regular node
/// and an absent entry carry no mode evidence, and a refusal that is not
/// permission-denied is already classified.
#[test]
fn only_a_permission_denied_regular_entry_short_of_the_bits_is_reread() {
    let unrefined = [
        refine_open_refusal(
            permission_denied(CustodyOp::OpenLock),
            observed(NodeKind::Regular, 0o600),
            0o600,
        ),
        refine_open_refusal(
            permission_denied(CustodyOp::OpenLock),
            observed(NodeKind::Regular, 0o644),
            0o600,
        ),
        refine_open_refusal(
            permission_denied(CustodyOp::OpenLock),
            observed(NodeKind::Other, 0o000),
            0o600,
        ),
        refine_open_refusal(permission_denied(CustodyOp::OpenLock), None, 0o600),
        refine_open_refusal(
            CustodyError::NotFound {
                op: CustodyOp::OpenLock,
            },
            observed(NodeKind::Regular, 0o000),
            0o600,
        ),
    ];
    for refusal in unrefined {
        assert!(
            !matches!(refusal, CustodyError::ModeDenied { .. }),
            "{refusal:?} was reread as a mode refusal"
        );
    }
}
