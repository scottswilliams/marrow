//! A presence guard must execute every key load and its producer before its consumer.

use super::Verdict::{Refused, Verified};
use super::{
    KEY_ID, add_fn, durable_schema, durable_schema_with_keys, finish_two_key, ok, verdict_of,
};
use marrow_image::{
    DraftTxn, ExportId, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, Scalar,
};
use marrow_verify::VerifyPhase;

fn finish_presence_export(
    mut draft: DraftTxn<'_>,
    params: Vec<ImageType>,
    local_count: u16,
    code: Vec<Instr>,
) -> Vec<u8> {
    let func = add_fn(
        &mut draft,
        "put",
        params,
        ImageType::Unit,
        local_count,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

/// Which durable operation establishes the presence fact the strict set then relies on.
#[derive(Clone, Copy, Debug)]
enum Guard {
    Exists,
    ReadEntry,
    Create,
}

/// What the arm the guard is not reached through contributes. `Divergent` loads a key of
/// its own, so the guard's first key operand has two producers and no single dominating
/// load; `Neutral` is stack-neutral and leaves the one key load after the join.
#[derive(Clone, Copy, Debug)]
enum Arm {
    Divergent,
    Neutral,
}

/// A guarded strict sparse set whose guard is reached through a two-armed branch, over the
/// tracer root keyed by one `text` column or by the composite `(int, text)` pair.
///
/// Instruction indices are computed from the piece lengths rather than written out, so the
/// arms, the join, and the guard's own branch target stay consistent across the shapes.
fn presence_image(guard: Guard, composite: bool, arm: Arm) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = if composite {
        durable_schema_with_keys(
            &mut draft,
            vec![
                KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(KEY_ID),
                },
                KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes([0x1c; 16]),
                },
            ],
        )
    } else {
        durable_schema(&mut draft)
    };
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));

    let mut code = vec![Instr::TxnBegin];
    if matches!(guard, Guard::Create) {
        // Slot 2 holds the entry record, so the create matches the `LocalGet(rec);
        // LocalGet(key)` shape the presence lattice keys on.
        let zero = ok(draft.intern_int(0));
        code.extend([
            Instr::ConstLoad(zero),
            Instr::ConstLoad(text),
            Instr::SomeWrap,
            Instr::RecordNew(sites.record),
            Instr::LocalSet(2),
        ]);
    }

    // The join is where both arms meet; the guard reads its key operands from there on.
    let branch_at = code.len();
    let (then_arm, else_arm) = match arm {
        // The wrong key is the one slot the right arm never loads: slot 1 for a single
        // column, slot 2 for the composite pair.
        Arm::Divergent => (
            vec![Instr::LocalGet(if composite { 2 } else { 1 })],
            vec![Instr::LocalGet(0)],
        ),
        Arm::Neutral => (vec![Instr::ConstLoad(flag), Instr::Pop], Vec::new()),
    };
    let divergent = matches!(arm, Arm::Divergent);
    let else_at = branch_at + 2 + then_arm.len() + usize::from(divergent);
    let join_at = else_at + else_arm.len();
    code.push(Instr::ConstLoad(flag));
    code.push(Instr::JumpIfFalse(else_at as u32));
    code.extend(then_arm);
    if divergent {
        code.push(Instr::Jump(join_at as u32));
    }
    code.extend(else_arm);
    if matches!(arm, Arm::Neutral) {
        code.push(Instr::LocalGet(0));
    }
    if composite {
        code.push(Instr::LocalGet(1));
    }
    if matches!(guard, Guard::Create) {
        code.push(Instr::LocalGet(2));
    }

    // The set is two instructions, and a presence branch skips over it to the commit.
    let set_at = code.len()
        + match guard {
            Guard::Exists => 2,
            Guard::ReadEntry => 3,
            Guard::Create => 1,
        };
    let commit_at = (set_at + 2) as u32;
    match guard {
        Guard::Exists => {
            code.extend([Instr::DurExists(sites.entry), Instr::JumpIfFalse(commit_at)])
        }
        Guard::ReadEntry => code.extend([
            Instr::DurReadEntry(sites.entry),
            Instr::BranchPresent(commit_at),
            Instr::Pop,
        ]),
        Guard::Create => code.push(Instr::DurCreateEntry(sites.entry)),
    }
    code.extend([
        Instr::ConstLoad(text),
        Instr::DurSetField {
            site: sites.label,
            key_slots: if composite { vec![0, 1] } else { vec![0] },
        },
        Instr::TxnCommit,
        Instr::Return,
    ]);

    if composite {
        finish_presence_export(
            draft,
            vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Text),
                ImageType::scalar(Scalar::Int),
            ],
            3,
            code,
        )
    } else if matches!(guard, Guard::Create) {
        finish_presence_export(
            draft,
            vec![
                ImageType::scalar(Scalar::Text),
                ImageType::scalar(Scalar::Text),
            ],
            3,
            code,
        )
    } else {
        finish_two_key(draft, code)
    }
}

/// The product of the three presence-establishing operations with the single and composite
/// key shapes: a divergent arm denies the guard a dominating key load and must be refused
/// at the flow phase, while the stack-neutral arm leaves the one key load dominating and
/// must verify. Composite creates are covered by the off-product pins below.
#[test]
fn a_presence_guard_needs_one_dominating_producer_per_key() {
    for guard in [Guard::Exists, Guard::ReadEntry, Guard::Create] {
        for composite in [false, true] {
            if composite && matches!(guard, Guard::Create) {
                continue;
            }
            for (arm, want) in [
                (Arm::Divergent, Refused(VerifyPhase::Flow)),
                (Arm::Neutral, Verified),
            ] {
                assert_eq!(
                    verdict_of(&presence_image(guard, composite, arm)),
                    want,
                    "{guard:?} over a {} key with a {arm:?} arm",
                    if composite { "composite" } else { "single" },
                );
            }
        }
    }
}

#[test]
fn guard_provenance_rejects_entry_at_exists_conditional() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::ConstLoad(flag),        // 3
            Instr::Jump(7),                // 4
            Instr::LocalGet(0),            // 5
            Instr::DurExists(sites.entry), // 6
            Instr::JumpIfFalse(10),        // 7
            Instr::ConstLoad(text),        // 8
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 9
            Instr::TxnCommit,              // 10
            Instr::Return,                 // 11
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

#[test]
fn guard_provenance_rejects_entry_at_read_entry_conditional() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,                          // 0
            Instr::ConstLoad(flag),                   // 1
            Instr::JumpIfFalse(6),                    // 2
            Instr::LocalGet(1),                       // 3
            Instr::DurReadEntry(sites.entry.clone()), // 4
            Instr::Jump(8),                           // 5
            Instr::LocalGet(0),                       // 6
            Instr::DurReadEntry(sites.entry),         // 7
            Instr::BranchPresent(12),                 // 8
            Instr::Pop,                               // 9
            Instr::ConstLoad(text),                   // 10
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 11
            Instr::TxnCommit,                         // 12
            Instr::Return,                            // 13
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

#[test]
fn guard_provenance_rejects_entry_at_create() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let zero = ok(draft.intern_int(0));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        3,
        vec![
            Instr::TxnBegin,                    // 0
            Instr::ConstLoad(zero),             // 1
            Instr::ConstLoad(text),             // 2
            Instr::SomeWrap,                    // 3
            Instr::RecordNew(sites.record),     // 4
            Instr::LocalSet(2),                 // 5
            Instr::ConstLoad(flag),             // 6
            Instr::JumpIfFalse(11),             // 7
            Instr::LocalGet(1),                 // 8
            Instr::LocalGet(2),                 // 9
            Instr::Jump(13),                    // 10
            Instr::LocalGet(0),                 // 11
            Instr::LocalGet(2),                 // 12
            Instr::DurCreateEntry(sites.entry), // 13
            Instr::ConstLoad(text),             // 14
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 15
            Instr::TxnCommit,                   // 16
            Instr::Return,                      // 17
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

#[test]
fn guard_provenance_rejects_late_backward_entry_with_unchanged_frame() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = durable_schema(&mut draft);
    let falseflag = ok(draft.intern_bool(false));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(falseflag),   // 1
            Instr::JumpIfFalse(10),        // 2
            Instr::LocalGet(0),            // 3
            Instr::DurExists(sites.entry), // 4
            Instr::JumpIfFalse(8),         // 5
            Instr::ConstLoad(text),        // 6
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 7
            Instr::TxnCommit,              // 8
            Instr::Return,                 // 9
            Instr::LocalGet(1),            // 10
            Instr::Jump(4),                // 11
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

#[test]
fn an_exists_guard_intersects_adjacent_successor_facts() {
    for (absent_target, expected) in [(4, Refused(VerifyPhase::Flow)), (6, Verified)] {
        let mut draft_owner = ImageDraft::new();
        let mut draft = draft_owner.begin_transaction();
        let sites = durable_schema(&mut draft);
        let text = ok(draft.intern_text("x"));
        // Target 4 merges the absent and present edges before the strict use.
        // Target 6 lets the absent edge commit and return without reaching it.
        let bytes = finish_two_key(
            draft,
            vec![
                Instr::TxnBegin,                   // 0
                Instr::LocalGet(0),                // 1
                Instr::DurExists(sites.entry),     // 2
                Instr::JumpIfFalse(absent_target), // 3
                Instr::ConstLoad(text),            // 4
                Instr::DurSetField {
                    site: sites.label,
                    key_slots: vec![0],
                }, // 5
                Instr::TxnCommit,                  // 6
                Instr::Return,                     // 7
            ],
        );
        assert_eq!(
            verdict_of(&bytes),
            expected,
            "absent target {absent_target}"
        );
    }
}

#[test]
fn a_late_backedge_rechecks_presence_at_an_already_visited_strict_use() {
    #[derive(Debug)]
    enum Backedge {
        Erase,
        Rebind,
        Preserve,
    }

    for backedge in [Backedge::Erase, Backedge::Rebind, Backedge::Preserve] {
        let mut draft_owner = ImageDraft::new();
        let mut draft = draft_owner.begin_transaction();
        let sites = durable_schema(&mut draft);
        let flag = ok(draft.intern_bool(false));
        let text = ok(draft.intern_text("x"));
        let (step, expected) = match &backedge {
            Backedge::Erase => (
                Instr::DurEraseEntry(sites.entry.clone()),
                Refused(VerifyPhase::Flow),
            ),
            Backedge::Rebind => (Instr::LocalSet(0), Refused(VerifyPhase::Flow)),
            Backedge::Preserve => (Instr::Pop, Verified),
        };
        // Successors are queued target-first and popped last-first: the strict
        // use at 7 is checked before the late arm at 10. Edge 12 -> 4 has the
        // same empty stack and initialized local types in all three cases;
        // only presence loss requires another visit to the strict use.
        let bytes = finish_two_key(
            draft,
            vec![
                Instr::TxnBegin,               // 0
                Instr::LocalGet(0),            // 1
                Instr::DurExists(sites.entry), // 2
                Instr::JumpIfFalse(13),        // 3
                Instr::ConstLoad(flag),        // 4
                Instr::JumpIfFalse(10),        // 5
                Instr::ConstLoad(text),        // 6
                Instr::DurSetField {
                    site: sites.label,
                    key_slots: vec![0],
                }, // 7
                Instr::TxnCommit,              // 8
                Instr::Return,                 // 9
                Instr::LocalGet(1),            // 10
                step,                          // 11
                Instr::Jump(4),                // 12
                Instr::TxnCommit,              // 13
                Instr::Return,                 // 14
            ],
        );
        assert_eq!(verdict_of(&bytes), expected, "{backedge:?}");
    }
}
