//! Phase 3+ call-graph and presence-flow checking over sealed functions.

use super::context::{Ctx, Effects};
use super::decode_code::decode_code;
use super::decode_code::resolve_jumps;
use super::flow::{branch_key_columns, check_flow};
use super::model::{DecodedFunction, DecodedImage};
use super::reject;
use super::spans::map_spans;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{SealedFunction, SealedInstr, SealedSite, SealedSiteTarget};
use marrow_image::{OperationClass, SemanticPath};
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "presence/family_lookup_tests.rs"]
mod family_lookup_tests;

#[cfg(test)]
#[path = "../../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
#[cfg(test)]
#[path = "presence/retention_tests.rs"]
mod retention_tests;
#[cfg(test)]
#[path = "../../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;

/// The validated entry sites indexed by canonical path for the presence phase.
/// Rows borrow both paths and branch coordinates; no key-column reconstruction is
/// needed to invalidate a family. The index is built once and is not published.
pub(super) struct EntryFamilies<'a> {
    rows: Vec<(&'a SemanticPath, u16, &'a [u16])>,
}

impl<'a> EntryFamilies<'a> {
    pub(super) fn new(sites: &'a [SealedSite], paths: &'a [SemanticPath]) -> Self {
        let mut rows = Vec::new();
        for (site, path) in sites.iter().zip(paths) {
            if let Some((root, branch)) = entry_family(site) {
                rows.push((path, root, branch));
            }
        }
        rows.sort_unstable_by(|left, right| left.0.cmp(right.0));
        #[cfg(test)]
        family_lookup_tests::record_build(rows.len(), rows.capacity());
        Self { rows }
    }

    fn get(&self, path: &SemanticPath) -> Option<(u16, &'a [u16])> {
        #[cfg(test)]
        family_lookup_tests::record_lookup();
        self.rows
            .binary_search_by(|(candidate, _, _)| {
                #[cfg(test)]
                family_lookup_tests::record_comparison();
                candidate.cmp(&path)
            })
            .ok()
            .map(|index| {
                let (_, root, branch) = self.rows[index];
                (root, branch)
            })
    }
}

/// Phase 4: reject any cycle in the direct-call graph (recursion is not admitted).
/// A three-colour DFS over the recorded calls; a back edge to a node on the current
/// stack is a cycle.
pub(super) fn reject_call_cycles(functions: &[SealedFunction]) -> Result<(), VerifyRejection> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Colour {
        White,
        Gray,
        Black,
    }
    let mut colour = vec![Colour::White; functions.len()];
    // Iterative DFS: a frame is (node, next-child-cursor).
    for start in 0..functions.len() {
        if colour[start] != Colour::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        colour[start] = Colour::Gray;
        while let Some(&(node, cursor)) = stack.last() {
            let callees: Vec<usize> = call_targets(&functions[node]);
            if cursor < callees.len() {
                stack.last_mut().expect("frame present").1 += 1;
                let next = callees[cursor];
                match colour[next] {
                    Colour::Gray => {
                        return Err(reject(
                            VerifyPhase::Closure,
                            "the call graph contains a cycle",
                        ));
                    }
                    Colour::White => {
                        colour[next] = Colour::Gray;
                        stack.push((next, 0));
                    }
                    Colour::Black => {}
                }
            } else {
                colour[node] = Colour::Black;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// The direct-call targets of a sealed function, in tape order.
pub(super) fn call_targets(function: &SealedFunction) -> Vec<usize> {
    function
        .instrs()
        .iter()
        .filter_map(|instr| match instr {
            SealedInstr::Call(target) => Some(*target as usize),
            _ => None,
        })
        .collect()
}

/// The control-flow successors of the sealed instruction at `index`.
pub(super) fn flow_successors(code: &[SealedInstr], index: usize) -> Vec<usize> {
    match &code[index] {
        SealedInstr::Return | SealedInstr::Unreachable(_) | SealedInstr::Todo(_) => Vec::new(),
        SealedInstr::Jump(target) => vec![*target],
        SealedInstr::JumpIfFalse(target)
        | SealedInstr::BranchPresent(target)
        | SealedInstr::IntAddChecked(target)
        | SealedInstr::IntSubChecked(target)
        | SealedInstr::IntMulChecked(target)
        | SealedInstr::IntNegChecked(target)
        | SealedInstr::IntDivChecked(target)
        | SealedInstr::IntRemChecked(target) => {
            vec![*target, index + 1]
        }
        _ => vec![index + 1],
    }
}

/// Phase 5 (presence): the place-slot presence lattice. A present-form
/// instruction — field set, group read or replacement through place slots — asserts
/// its containing entry is present; this recheck proves that independently of the
/// compiler, so a forged or mis-lowered present-form op whose graph cannot imply its
/// payload is refused.
///
/// The lattice state at each program point is the set of proven-present entries, each
/// its family and its key-path slots. A fact is *established* by a guard that tests the
/// entry keyed by the slots — `LocalGet(S…); DurExists(entry); JumpIfFalse` on its
/// present (fallthrough) edge, or an optional entry/group read followed by `BranchPresent` on its
/// present edge — or by a whole-entry `DurCreateEntry` keyed by those slots (create
/// leaves the entry present whether it was created or already present). It is *killed*
/// by any entry erase of the fact's family, whatever key the erase names; by a call
/// whose demand closure erases an entry of the family; and by any `LocalSet` of a slot the fact
/// reads (a `place` key slot is bind-once, so a rebind never fires on compiler output —
/// it hardens the recheck against a mutated tape). Facts join by intersection at
/// merges: an entry is present only if it holds on every incoming edge.
pub(super) fn check_presence_flow(
    function: &SealedFunction,
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    effects: &Effects,
    entry_families: &EntryFamilies<'_>,
) -> Result<(), VerifyRejection> {
    let code = function.instrs();
    if !code.iter().any(|instr| {
        matches!(
            instr,
            SealedInstr::DurSetField { .. }
                | SealedInstr::DurReadFieldPresent { .. }
                | SealedInstr::DurReadGroupPresent { .. }
                | SealedInstr::DurReplaceGroup { .. }
        )
    }) {
        return Ok(());
    }
    let mut entry: Vec<Option<BTreeSet<PresenceFact>>> = vec![None; code.len()];
    entry[0] = Some(BTreeSet::new());
    let mut worklist = vec![0usize];
    while let Some(mut index) = worklist.pop() {
        let mut present = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        'linear: loop {
            if let SealedInstr::DurSetField { site, key_slots }
            | SealedInstr::DurReadFieldPresent { site, key_slots }
            | SealedInstr::DurReadGroupPresent { site, key_slots }
            | SealedInstr::DurReplaceGroup { site, key_slots } = &code[index]
            {
                // The present form is proven only if a dominating fact names the exact
                // containing entry — its family and its whole key-path — not merely a
                // matching slot tuple (sibling branches of equal arity share slot tuples).
                let (root, branch) = payload_site_family(ctx, *site).ok_or(reject(
                    VerifyPhase::Flow,
                    "a present-entry operation does not resolve to a field or group site",
                ))?;
                if !present.contains(&(root, branch, key_slots.clone())) {
                    return Err(reject(
                        VerifyPhase::Flow,
                        "a present-entry operation is not dominated by a presence fact on its containing entry",
                    ));
                }
            }
            let edges = presence_edges(
                code,
                ctx,
                non_fallthrough_entries,
                effects,
                entry_families,
                index,
                present,
            );
            // An adjacent explicit target is unmarked, but coincident fork
            // edges must still meet both sets.
            let coincident = matches!(
                edges.as_slice(),
                [(left, _), (right, _)] if left == right
            );
            for (successor, set) in edges {
                if !coincident
                    && successor == index + 1
                    && successor < code.len()
                    && !non_fallthrough_entries[successor]
                {
                    (index, present) = (successor, set);
                    continue 'linear;
                }
                if successor >= code.len() {
                    return Err(reject(VerifyPhase::Flow, "presence edge out of range"));
                }
                match &mut entry[successor] {
                    None => {
                        entry[successor] = Some(set);
                        worklist.push(successor);
                    }
                    Some(existing) => {
                        let merged: BTreeSet<PresenceFact> =
                            existing.intersection(&set).cloned().collect();
                        if merged.len() != existing.len() {
                            *existing = merged;
                            worklist.push(successor);
                        }
                    }
                }
            }
            break;
        }
    }
    #[cfg(test)]
    retention_tests::record_success(&entry, entry.capacity());
    Ok(())
}

/// A proven-present containing entry in the presence-flow lattice: the root index it
/// lives under, the entry's branch path (empty for the root itself), and its whole
/// key-path as pre-evaluated local slots (root-first). Keying on the root — not the
/// slot tuple or branch path alone — keeps entries under distinct roots distinct even
/// when they share a key slot; keying on the branch path distinguishes sibling
/// branches of equal key arity that share slot values under one root. The first two
/// components are the entry's *family*, the unit an erase or an entry-erasing call
/// ends proofs over.
type PresenceFact = (u16, Vec<u16>, Vec<u16>);

/// Consume the working set into successor states, cloning only for a fork.
/// Most instructions pass the set through unchanged; guards split the set (adding the
/// proven entry only on the present edge); create adds; an erase, an entry-erasing
/// call, and a slot rebind remove.
fn presence_edges(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    effects: &Effects,
    entry_families: &EntryFamilies<'_>,
    index: usize,
    mut present: BTreeSet<PresenceFact>,
) -> Vec<(usize, BTreeSet<PresenceFact>)> {
    match &code[index] {
        SealedInstr::JumpIfFalse(target) => {
            let mut present_edge = present.clone();
            if let Some(fact) = exists_guard_fact(code, ctx, non_fallthrough_entries, index) {
                present_edge.insert(fact);
            }
            vec![(*target, present), (index + 1, present_edge)]
        }
        SealedInstr::BranchPresent(target) => {
            let mut present_edge = present.clone();
            if let Some(fact) = read_entry_guard_fact(code, ctx, non_fallthrough_entries, index) {
                present_edge.insert(fact);
            }
            vec![(*target, present), (index + 1, present_edge)]
        }
        SealedInstr::DurCreateEntry(site) => {
            if let Some((root, branch, arity)) = entry_site(ctx, *site)
                && let Some(keys) =
                    entry_write_key_slots(code, non_fallthrough_entries, index, arity)
            {
                present.insert((root, branch, keys));
            }
            vec![(index + 1, present)]
        }
        SealedInstr::DurEraseEntry(site) => {
            // An entry erase ends every fact of the erased family whatever key operand
            // it names — a slot, another slot, or a constant — because the lattice does
            // not reason about key equality. Facts of other families survive: an erase
            // touches only the entry's own payload, so a child family's entry outlives
            // its parent's erase.
            if let Some((root, branch)) = ctx.sites.get(*site as usize).and_then(entry_family) {
                present.retain(|(fact_root, fact_branch, _)| {
                    (*fact_root, fact_branch.as_slice()) != (root, branch)
                });
            }
            vec![(index + 1, present)]
        }
        SealedInstr::Call(callee) => {
            // Only entry erasure removes a presence fact. Complete replacement
            // and field/group updates preserve the entry marker.
            for atom in &effects.atoms_closure[*callee as usize] {
                if atom.class() != OperationClass::Erase {
                    continue;
                }
                if let Some((root, branch)) = entry_families.get(atom.path()) {
                    present.retain(|(fact_root, fact_branch, _)| {
                        (*fact_root, fact_branch.as_slice()) != (root, branch)
                    });
                }
            }
            vec![(index + 1, present)]
        }
        SealedInstr::LocalSet(slot) => {
            // A rebind of any key-path slot invalidates every fact that reads it.
            present.retain(|(_, _, keys)| !keys.contains(slot));
            vec![(index + 1, present)]
        }
        _ => {
            let mut successors = flow_successors(code, index);
            let Some(last) = successors.pop() else {
                return Vec::new();
            };
            let mut edges = Vec::with_capacity(successors.len() + 1);
            edges.extend(successors.into_iter().map(|next| (next, present.clone())));
            edges.push((last, present));
            edges
        }
    }
}

/// Exact entry identity; field, group and index sites are not entry families.
fn entry_family(site: &SealedSite) -> Option<(u16, &[u16])> {
    let SealedSite::Flat { root, target } = site else {
        return None;
    };
    match target {
        SealedSiteTarget::WholePayload => Some((*root, &[])),
        SealedSiteTarget::BranchEntry(branch) => Some((*root, branch)),
        _ => None,
    }
}

/// The containing entry a flat entry (whole-payload or branch-entry) `site` names: the
/// root index it lives under, its branch path (empty for the root), and its whole
/// key-path column arity. `None` for a non-entry site (a field leaf or index), which
/// names no entry to prove present.
fn entry_site(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>, usize)> {
    let (root_index, branch) = entry_family(ctx.sites.get(site as usize)?)?;
    let root = ctx.roots.get(root_index as usize)?;
    let extra = branch_key_columns(root, branch).ok()?;
    Some((root_index, branch.to_vec(), root.keys.len() + extra.len()))
}

/// The family of the entry a flat payload `site` belongs to: the root index and the
/// branch path of a field leaf (empty for a root field, the branch placement path for a
/// branch field), or the root and an empty path for a root-level group. `None` for a
/// site that is not a field or group.
fn payload_site_family(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>)> {
    let SealedSite::Flat { root, target } = ctx.sites.get(site as usize)? else {
        return None;
    };
    match target {
        SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::GroupEntry(_) => {
            Some((*root, Vec::new()))
        }
        SealedSiteTarget::BranchField { branch, .. } => Some((*root, branch.to_vec())),
        _ => None,
    }
}

/// The `arity` key-path slots pushed immediately before position `at` (root-first): each
/// must be a `LocalGet`, or the guard establishes no fact. `at` is the position
/// immediately after the key loads.
fn read_key_path_before(code: &[SealedInstr], at: usize, arity: usize) -> Option<Vec<u16>> {
    if arity == 0 || at < arity {
        return None;
    }
    let mut keys = Vec::with_capacity(arity);
    for offset in 0..arity {
        let SealedInstr::LocalGet(slot) = &code[at - arity + offset] else {
            return None;
        };
        keys.push(*slot);
    }
    Some(keys)
}

/// Entry at the first load is valid; entry into each later instruction must come
/// from its immediate predecessor. Type flow supplies the mask for this tape.
fn window_has_only_fallthrough(entries: &[bool], first: usize, last: usize) -> bool {
    !entries[first + 1..=last].iter().any(|entry| *entry)
}

/// The presence fact an `exists`-guard proves at a `JumpIfFalse`: `LocalGet(S0); …;
/// LocalGet(Sn); DurExists(entry site); JumpIfFalse`. The fact is the entry site's
/// branch path paired with the whole key-path it reads. `None` when the shape does not
/// match (a non-entry site, a non-local key, or an unrelated condition).
fn exists_guard_fact(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    index: usize,
) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurExists(site) = &code[index - 1] else {
        return None;
    };
    let (root, branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    if !window_has_only_fallthrough(non_fallthrough_entries, index - arity - 1, index) {
        return None;
    }
    Some((root, branch, keys))
}

/// An optional entry or group read proves its containing entry at `BranchPresent`
/// only through the complete `LocalGet(keys...); read; BranchPresent` window.
fn read_entry_guard_fact(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    index: usize,
) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let (root, branch, arity) = match &code[index - 1] {
        SealedInstr::DurReadEntry(site) => entry_site(ctx, *site)?,
        SealedInstr::DurReadGroup(site) => {
            let SealedSite::Flat {
                root,
                target: SealedSiteTarget::GroupEntry(_),
            } = ctx.sites.get(usize::from(*site))?
            else {
                return None;
            };
            (
                *root,
                Vec::new(),
                ctx.roots.get(usize::from(*root))?.keys.len(),
            )
        }
        _ => return None,
    };
    let keys = read_key_path_before(code, index - 1, arity)?;
    if !window_has_only_fallthrough(non_fallthrough_entries, index - arity - 1, index) {
        return None;
    }
    Some((root, branch, keys))
}

/// The whole key tuple below a locally loaded create record. Every instruction
/// after the first key load must execute by fallthrough through the create.
fn entry_write_key_slots(
    code: &[SealedInstr],
    non_fallthrough_entries: &[bool],
    index: usize,
    arity: usize,
) -> Option<Vec<u16>> {
    let record_at = index.checked_sub(1)?;
    let SealedInstr::LocalGet(_) = &code[record_at] else {
        return None;
    };
    let keys = read_key_path_before(code, record_at, arity)?;
    if !window_has_only_fallthrough(non_fallthrough_entries, record_at - arity, index) {
        return None;
    }
    Some(keys)
}

pub(super) fn verify_function(
    function: &DecodedFunction,
    ctx: &Ctx,
    decoded: &DecodedImage,
) -> Result<(SealedFunction, Vec<bool>), VerifyRejection> {
    let mut decoded_code = decode_code(&function.code)?;
    let non_fallthrough_entries = resolve_jumps(&mut decoded_code)?;
    let (instrs, max_stack) = check_flow(
        function,
        ctx,
        &decoded_code,
        &decoded.consts,
        &non_fallthrough_entries,
    )?;
    let spans = map_spans(function, &decoded_code)?;
    Ok((
        SealedFunction {
            name: decoded.strings[function.name as usize].clone(),
            source: decoded.strings[function.source as usize].clone(),
            params: function.params.clone(),
            ret: function.ret,
            local_count: function.local_count,
            instrs,
            spans,
            max_stack,
            mutating: false,
        },
        non_fallthrough_entries,
    ))
}

#[cfg(test)]
mod presence_root_discrimination {
    //! The presence lattice keys a proven-present entry on its root, not on its
    //! key slot alone. Two whole-entry creates that share a key slot but address
    //! different roots must establish two distinct facts, so a strict sparse set
    //! over one root can never be proven by a create on another.
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use marrow_image::Scalar;

    use super::super::context::{Ctx, Effects};
    use super::{EntryFamilies, presence_edges};
    use crate::sealed::{SealedInstr, SealedRoot, SealedSite, SealedSiteTarget};

    fn keyed_root(name: &str) -> SealedRoot {
        SealedRoot {
            name: Rc::from(name),
            keys: vec![Scalar::Int],
            record: 0,
            has_extras: false,
            branches: Vec::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn two_root_creates_sharing_a_slot_establish_distinct_facts() {
        let roots = [keyed_root("assets"), keyed_root("tallies")];
        let sites = [
            SealedSite::Flat {
                root: 0,
                target: SealedSiteTarget::WholePayload,
            },
            SealedSite::Flat {
                root: 1,
                target: SealedSiteTarget::WholePayload,
            },
        ];
        let ctx = Ctx {
            types: &[],
            enums: &[],
            collections: &[],
            roots: &roots,
            sites: &sites,
            indexes: &[],
            signatures: &[],
        };
        // `LocalGet(key); LocalGet(record); DurCreateEntry(site)` twice, the two
        // creates addressing different roots through the SAME key slot (7).
        let code = [
            SealedInstr::LocalGet(7),
            SealedInstr::LocalGet(3),
            SealedInstr::DurCreateEntry(0),
            SealedInstr::LocalGet(7),
            SealedInstr::LocalGet(3),
            SealedInstr::DurCreateEntry(1),
        ];
        let entries = [false; 6];
        let effects = Effects::compute(&[], &[]);
        let families = EntryFamilies::new(&[], &[]);
        let after_first = presence_edges(
            &code,
            &ctx,
            &entries,
            &effects,
            &families,
            2,
            BTreeSet::new(),
        )
        .into_iter()
        .find(|(successor, _)| *successor == 3)
        .expect("a create falls through to the next instruction")
        .1;
        let after_second =
            presence_edges(&code, &ctx, &entries, &effects, &families, 5, after_first)
                .into_iter()
                .find(|(successor, _)| *successor == 6)
                .expect("a create falls through to the next instruction")
                .1;
        assert_eq!(
            after_second.len(),
            2,
            "creates on distinct roots must not alias to one presence fact",
        );
    }
}
