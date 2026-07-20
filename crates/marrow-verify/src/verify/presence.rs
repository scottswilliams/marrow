//! Phase 3+ call-graph and presence-flow checking over sealed functions.

use super::context::Ctx;
use super::decode_code::decode_code;
use super::decode_code::resolve_jumps;
use super::flow::{Frame, branch_key_columns, check_flow};
use super::model::{DecodedFunction, DecodedImage};
use super::reject;
use super::spans::map_spans;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{SealedFunction, SealedInstr, SealedSite, SealedSiteTarget};
use crate::vtype::VType;
use std::collections::BTreeSet;

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

/// Phase 5 (presence): the place-slot presence lattice (design §D). A
/// `DurSetSparsePresent` (the strict sparse set) asserts its containing entry is
/// present; this recheck proves that independently of the compiler, so a forged or
/// mis-lowered strict set whose graph cannot imply its payload is refused.
///
/// The lattice state at each program point is the set of key-slot locals whose entry
/// a dominating fact has proven present. A fact is *established* by a guard that
/// tests the entry keyed by a slot — `LocalGet(S); DurExists(entry); JumpIfFalse` on
/// its present (fallthrough) edge, or `LocalGet(S); DurReadEntry; BranchPresent` on
/// its present edge — or by a whole-entry `DurCreateEntry` keyed by that slot (create
/// leaves the entry present whether it was created or already present). It is
/// *killed* by an entry erase keyed by the slot or by any `LocalSet` of the slot (a
/// rebind; a `place` key slot is bind-once, so this never fires on compiler output —
/// it hardens the recheck against a mutated tape). Facts join by intersection at
/// merges: a slot is present only if it holds on every incoming edge. Calls are
/// transparent (no aliasing model): a mutation reached through a call that erases the
/// entry is caught by the kernel's runtime presence assertion, not here.
pub(super) fn check_presence_flow(
    function: &SealedFunction,
    ctx: &Ctx,
) -> Result<(), VerifyRejection> {
    let code = function.instrs();
    if !code
        .iter()
        .any(|instr| matches!(instr, SealedInstr::DurSetSparsePresent { .. }))
    {
        return Ok(());
    }
    let mut entry: Vec<Option<BTreeSet<PresenceFact>>> = vec![None; code.len()];
    entry[0] = Some(BTreeSet::new());
    let mut worklist = vec![0usize];
    while let Some(index) = worklist.pop() {
        let present = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        if let SealedInstr::DurSetSparsePresent { site, key_slots } = &code[index] {
            // The strict set is proven only if a dominating fact names the exact
            // containing entry — its branch path and its whole key-path — not merely a
            // matching slot tuple (sibling branches of equal arity share slot tuples).
            let (root, branch) = field_site_branch_path(ctx, *site).ok_or(reject(
                VerifyPhase::Flow,
                "a present-entry sparse set does not resolve to a field site",
            ))?;
            if !present.contains(&(root, branch, key_slots.clone())) {
                return Err(reject(
                    VerifyPhase::Flow,
                    "a present-entry sparse set is not dominated by a presence fact on its containing entry",
                ));
            }
        }
        for (successor, set) in presence_edges(code, ctx, index, &present) {
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
    }
    Ok(())
}

/// A proven-present containing entry in the presence-flow lattice: the root index it
/// lives under, the entry's branch path (empty for the root itself), and its whole
/// key-path as pre-evaluated local slots (root-first). Keying on the root — not the
/// slot tuple or branch path alone — keeps entries under distinct roots distinct even
/// when they share a key slot; keying on the branch path distinguishes sibling
/// branches of equal key arity that share slot values under one root.
type PresenceFact = (u16, Vec<u16>, Vec<u16>);

/// The presence-set carried on each successor edge of the instruction at `index`.
/// Most instructions pass the set through unchanged; guards split the set (adding the
/// proven slot only on the present edge); create adds and erase/rebind remove.
fn presence_edges(
    code: &[SealedInstr],
    ctx: &Ctx,
    index: usize,
    present: &BTreeSet<PresenceFact>,
) -> Vec<(usize, BTreeSet<PresenceFact>)> {
    match &code[index] {
        SealedInstr::JumpIfFalse(target) => match exists_guard_fact(code, ctx, index) {
            // The present (true) edge falls through into the guarded block; the false
            // edge (target) is the absent branch.
            Some(fact) => {
                let mut present_edge = present.clone();
                present_edge.insert(fact);
                vec![(*target, present.clone()), (index + 1, present_edge)]
            }
            None => flow_successors(code, index)
                .into_iter()
                .map(|s| (s, present.clone()))
                .collect(),
        },
        SealedInstr::BranchPresent(target) => match read_entry_guard_fact(code, ctx, index) {
            Some(fact) => {
                let mut present_edge = present.clone();
                present_edge.insert(fact);
                vec![(*target, present.clone()), (index + 1, present_edge)]
            }
            None => flow_successors(code, index)
                .into_iter()
                .map(|s| (s, present.clone()))
                .collect(),
        },
        SealedInstr::DurCreateEntry(site) => {
            let mut next = present.clone();
            // Only a single-column root whole-entry create establishes root-entry
            // presence for its key slot (`entry_write_key_slot` reads one adjacent key).
            // A branch create (a `BranchEntry` site) leaves the root descendant-only, and
            // a composite-root create's misread single slot never matches a set's full
            // key-path — so neither falsely establishes a fact a strict set relies on.
            if is_entry_site(ctx, *site)
                && let Some(root) = site_root(ctx, *site)
                && let Some(slot) = entry_write_key_slot(code, index)
            {
                next.insert((root, Vec::new(), vec![slot]));
            }
            vec![(index + 1, next)]
        }
        SealedInstr::DurEraseEntry(site) => {
            let mut next = present.clone();
            // An entry erase whose whole key-path is pre-evaluated slots kills that exact
            // entry's presence fact (its root, branch path, and key-path): a root erase
            // kills the root fact, a branch erase kills only its own branch entry.
            if let Some((root, branch, arity)) = entry_site(ctx, *site)
                && let Some(keys) = read_key_path_before(code, index, arity)
            {
                next.remove(&(root, branch, keys));
            }
            vec![(index + 1, next)]
        }
        SealedInstr::LocalSet(slot) => {
            let mut next = present.clone();
            // A rebind of any key-path slot invalidates every fact that reads it.
            next.retain(|(_, _, keys)| !keys.contains(slot));
            vec![(index + 1, next)]
        }
        _ => flow_successors(code, index)
            .into_iter()
            .map(|s| (s, present.clone()))
            .collect(),
    }
}

/// Whether `site` is a flat whole-payload (entry marker) site — the presence a
/// containing-payload fact is about.
fn is_entry_site(ctx: &Ctx, site: u16) -> bool {
    matches!(
        ctx.sites.get(site as usize),
        Some(SealedSite::Flat {
            target: SealedSiteTarget::WholePayload,
            ..
        })
    )
}

/// The root index a flat `site` resolves to. `None` for a non-flat (parked) site.
pub(super) fn site_root(ctx: &Ctx, site: u16) -> Option<u16> {
    match ctx.sites.get(site as usize)? {
        SealedSite::Flat { root, .. } => Some(*root),
        SealedSite::Parked { .. } => None,
    }
}

/// The containing entry a flat entry (whole-payload or branch-entry) `site` names: the
/// root index it lives under, its branch path (empty for the root), and its whole
/// key-path column arity. `None` for a non-entry site (a field leaf or index), which
/// names no entry to prove present.
fn entry_site(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>, usize)> {
    let SealedSite::Flat {
        root: root_index,
        target,
    } = ctx.sites.get(site as usize)?
    else {
        return None;
    };
    let root = ctx.roots.get(*root_index as usize)?;
    match target {
        SealedSiteTarget::WholePayload => Some((*root_index, Vec::new(), root.keys.len())),
        SealedSiteTarget::BranchEntry(path) => {
            let extra = branch_key_columns(root, path).ok()?;
            Some((*root_index, path.to_vec(), root.keys.len() + extra.len()))
        }
        SealedSiteTarget::FieldLeaf(_)
        | SealedSiteTarget::BranchField { .. }
        | SealedSiteTarget::GroupEntry(_)
        | SealedSiteTarget::IndexScan(_)
        | SealedSiteTarget::IndexLookup(_) => None,
    }
}

/// The root index and branch path of a flat field-leaf `site`: the branch path is empty
/// for a root field, the branch placement path for a branch field. `None` for a
/// non-field site.
fn field_site_branch_path(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>)> {
    let SealedSite::Flat { root, target } = ctx.sites.get(site as usize)? else {
        return None;
    };
    match target {
        SealedSiteTarget::FieldLeaf(_) => Some((*root, Vec::new())),
        SealedSiteTarget::BranchField { branch, .. } => Some((*root, branch.to_vec())),
        _ => None,
    }
}

/// The `arity` key-path slots pushed immediately before position `at` (root-first): each
/// must be a `LocalGet`, or the guard establishes no fact. `at` is the position of the
/// consuming `DurExists`/`DurReadEntry`.
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

/// The presence fact an `exists`-guard proves at a `JumpIfFalse`: `LocalGet(S0); …;
/// LocalGet(Sn); DurExists(entry site); JumpIfFalse`. The fact is the entry site's
/// branch path paired with the whole key-path it reads. `None` when the shape does not
/// match (a non-entry site, a non-local key, or an unrelated condition).
fn exists_guard_fact(code: &[SealedInstr], ctx: &Ctx, index: usize) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurExists(site) = &code[index - 1] else {
        return None;
    };
    let (root, branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    Some((root, branch, keys))
}

/// The presence fact an `if const x = p` guard proves at a `BranchPresent`:
/// `LocalGet(S0); …; LocalGet(Sn); DurReadEntry(entry site); BranchPresent`.
fn read_entry_guard_fact(code: &[SealedInstr], ctx: &Ctx, index: usize) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurReadEntry(site) = &code[index - 1] else {
        return None;
    };
    let (root, branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    Some((root, branch, keys))
}

/// The key slot of a single-column whole-entry create at `index`: `LocalGet(S);
/// LocalGet(record); DurCreateEntry`. The key is the operand below the record, so the
/// create's key comes from the `LocalGet` two back when the record is a single local
/// push.
///
/// Soundness of shape-adjacent slot identification: the caller applies this only to a
/// root `WholePayload` create (it gates on `is_entry_site`), so a branch create — whose
/// key-path leaves a *branch* key adjacent to the op — never reaches here and never
/// establishes root-entry presence, and a composite-root create's misread single slot
/// forms a 1-tuple fact no full-key-path strict set ever matches. The caller pairs this
/// slot with the create's own root (`site_root`), so the established fact is keyed on
/// (root, slot): two writes through the same slot value under different roots establish
/// distinct facts, and a strict sparse set over one root is never proven by a create on
/// another.
fn entry_write_key_slot(code: &[SealedInstr], index: usize) -> Option<u16> {
    if index < 2 {
        return None;
    }
    let SealedInstr::LocalGet(_) = &code[index - 1] else {
        return None;
    };
    let SealedInstr::LocalGet(slot) = &code[index - 2] else {
        return None;
    };
    Some(*slot)
}

/// The successor edges for a two-way branch that keeps the current stack on the
/// `target` edge and pushes one value on the fallthrough edge (`index + 1`). Shared
/// by `BranchPresent` (present value) and the checked ops (int result).
pub(super) fn push_on_fallthrough(
    frame: &Frame,
    target: usize,
    index: usize,
    pushed: VType,
    max_stack: &mut usize,
) -> Result<Vec<(usize, Frame)>, VerifyRejection> {
    let mut fallthrough = frame.clone();
    fallthrough.stack.push(pushed);
    if fallthrough.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
        return Err(reject(
            VerifyPhase::Function,
            "operand stack exceeds depth bound",
        ));
    }
    *max_stack = (*max_stack).max(fallthrough.stack.len());
    Ok(vec![(target, frame.clone()), (index + 1, fallthrough)])
}

pub(super) fn verify_function(
    function: &DecodedFunction,
    ctx: &Ctx,
    decoded: &DecodedImage,
) -> Result<SealedFunction, VerifyRejection> {
    let mut decoded_code = decode_code(&function.code)?;
    resolve_jumps(&mut decoded_code)?;
    let (instrs, max_stack) = check_flow(function, ctx, &decoded_code, &decoded.consts)?;
    let spans = map_spans(function, &decoded_code)?;
    Ok(SealedFunction {
        name: decoded.strings[function.name as usize].clone(),
        source: decoded.strings[function.source as usize].clone(),
        params: function.params.clone(),
        ret: function.ret,
        local_count: function.local_count,
        instrs,
        spans,
        max_stack,
        mutating: false,
    })
}

#[cfg(test)]
mod presence_root_discrimination {
    //! The presence lattice keys a proven-present entry on its root, not on its
    //! key slot alone. Two whole-entry creates that share a key slot but address
    //! different roots must establish two distinct facts, so a strict sparse set
    //! over one root can never be proven by a create on another. This holds the
    //! (root, slot) discrimination structurally at the helper level, where it is
    //! observable even while the container bound admits a single root.
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use marrow_image::Scalar;

    use super::super::context::Ctx;
    use super::presence_edges;
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
        let after_first = presence_edges(&code, &ctx, 2, &BTreeSet::new())
            .into_iter()
            .find(|(successor, _)| *successor == 3)
            .expect("a create falls through to the next instruction")
            .1;
        let after_second = presence_edges(&code, &ctx, 5, &after_first)
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
