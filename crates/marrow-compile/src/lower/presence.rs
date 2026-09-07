//! Presence proofs over durable entries: the facts a guard or a whole-entry write
//! establishes, the loop regions and calls that end them, and the refusal of a
//! present-form write no fact covers.

use super::*;

/// A loop's patch targets — where `continue` jumps, and the jumps `break` emits that
/// must be patched to the loop's exit once it is known — and its proof region: a loop
/// body is one region, so a write through a fact older than the loop is refused when
/// the body erases the fact's family or calls a function that writes it, because the
/// back edge puts that erase before the write.
pub(super) struct LoopCtx<'a> {
    pub(super) continue_target: usize,
    pub(super) break_jumps: Vec<usize>,
    /// Families an entry erase inside this body (or a nested body) ended.
    pub(super) erased_families: Vec<&'a Family>,
    /// Functions called inside this body (or a nested body).
    pub(super) callees: Vec<u16>,
    /// Present-form writes inside this body through facts established before it was
    /// entered: the write's span and its family, resolved when the loop closes.
    pub(super) obligations: Vec<(SourceSpan, &'a Family)>,
}

impl<'a> LoopCtx<'a> {
    pub(super) fn new(continue_target: usize) -> Self {
        Self {
            continue_target,
            break_jumps: Vec::new(),
            erased_families: Vec::new(),
            callees: Vec::new(),
            obligations: Vec::new(),
        }
    }
}

/// A presence proof over one durable entry: the compiler knows the entry addressed by
/// `key_slots` (its whole key-path as pre-evaluated place slots, root-first) in
/// `family` is present from here to the end of the block that established the fact,
/// unless an erase of the family or a call that writes it ends it first. `depth` is
/// the loop nesting at establishment, so a write inside a loop entered later records
/// an obligation on that loop; `callees` are the functions called since establishment,
/// each checked after lowering against its written-family closure.
pub(super) struct PresenceFact<'a> {
    pub(super) family: &'a Family,
    pub(super) key_slots: Vec<u16>,
    pub(super) depth: usize,
    pub(super) callees: Vec<u16>,
}

/// A present-form write whose proof a call may have ended: the write's span, its
/// family, and the functions called between the proof and the write (or, for a write
/// inside a loop, inside that loop). Resolved after lowering, when every callee's
/// written-family closure is known.
pub(crate) struct PresenceObligation {
    pub span: SourceSpan,
    pub family: Family,
    pub callees: Vec<u16>,
}

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Record that the entry `key_slots` addresses in `family` is known present from
    /// here (a dominating guard or a completed upsert) until the enclosing block ends,
    /// the family is erased, or a call writes it. A fact re-established inside a loop
    /// is a fresh fact at the loop's depth, so a per-iteration guard is never charged
    /// with an outer fact's obligations.
    pub(super) fn mark_present(&mut self, family: &'a Family, key_slots: Vec<u16>) {
        self.present_places.push(PresenceFact {
            family,
            key_slots,
            depth: self.loops.len(),
            callees: Vec::new(),
        });
    }

    /// The newest presence fact over the entry `key_slots` addresses in `family`.
    pub(super) fn present_fact(
        &self,
        family: &Family,
        key_slots: &[u16],
    ) -> Option<&PresenceFact<'a>> {
        self.present_places
            .iter()
            .rev()
            .find(|fact| fact.family == family && fact.key_slots == key_slots)
    }

    /// Consume a presence proof for a present-form operation at `span`: the write is
    /// emitted only when a fact dominates the entry, and each loop entered after the
    /// fact was established records the write as an obligation it resolves when it
    /// closes, since its back edge may put an erase of the family before the write.
    /// A fact's callees carry into the post-lowering check the same way. Without a
    /// fact the write is refused here.
    pub(super) fn require_present(
        &mut self,
        family: &'a Family,
        key_slots: Option<Vec<u16>>,
        span: SourceSpan,
    ) -> ConstructResult<Vec<u16>> {
        let Some((key_slots, fact)) =
            key_slots.and_then(|slots| self.present_fact(family, &slots).map(|fact| (slots, fact)))
        else {
            self.fail(requires_presence(
                self.file,
                span,
                "no presence proof covers the entry here",
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let depth = fact.depth;
        if !fact.callees.is_empty() {
            self.presence_obligations.push(PresenceObligation {
                span,
                family: family.clone(),
                callees: fact.callees.clone(),
            });
        }
        for ctx in &mut self.loops[depth..] {
            ctx.obligations.push((span, family));
        }
        Ok(key_slots)
    }

    /// End every proof over `family`: its entry payload may be gone. Every open loop
    /// records the erase, so a write earlier in its body through an older fact is
    /// refused when the loop closes.
    pub(super) fn erase_family(&mut self, family: &'a Family) {
        self.present_places.retain(|fact| fact.family != family);
        for ctx in &mut self.loops {
            if !ctx.erased_families.contains(&family) {
                ctx.erased_families.push(family);
            }
        }
    }

    /// Record that this body creates, replaces, or erases an entry of `family`.
    pub(super) fn write_family(&mut self, family: &'a Family) {
        if !self.written_families.contains(&family) {
            self.written_families.push(family);
        }
    }

    /// Close a loop region: refuse every obligation whose family the body erased,
    /// carry the rest to the post-lowering callee check, and end every proof over an
    /// erased family for the code after the loop. Returns the loop's `break` jumps.
    pub(super) fn close_loop(&mut self, ctx: LoopCtx<'a>) -> Vec<usize> {
        for (span, family) in ctx.obligations {
            if ctx.erased_families.contains(&family) {
                self.fail(requires_presence(
                    self.file,
                    span,
                    "the loop body erases an entry of the family after this write, so \
                     the next iteration writes an entry the proof no longer covers",
                ));
            } else if !ctx.callees.is_empty() {
                self.presence_obligations.push(PresenceObligation {
                    span,
                    family: family.clone(),
                    callees: ctx.callees.clone(),
                });
            }
        }
        self.present_places
            .retain(|fact| !ctx.erased_families.contains(&fact.family));
        ctx.break_jumps
    }

    /// If `cond` is `exists(p)` over an in-scope named `place`, that place's presence
    /// fact key: the guarded (then) block may write through the place.
    pub(super) fn exists_guard_fact(&self, cond: &Expression) -> Option<(&'a Family, Vec<u16>)> {
        let Expression::Call { callee, args, .. } = cond else {
            return None;
        };
        let Expression::Name { segments, .. } = &**callee else {
            return None;
        };
        if !matches!(&segments[..], [callee] if callee.text() == "exists") {
            return None;
        }
        let [arg] = args.as_slice() else {
            return None;
        };
        if arg.name.is_some() {
            return None;
        }
        let Expression::Name { segments, .. } = &arg.value else {
            return None;
        };
        let [name] = &segments[..] else {
            return None;
        };
        self.lookup_place(name.text()).map(PlaceLocal::fact_key)
    }

    /// If `cond` is `not exists(p)` over an in-scope named `place`, the inner
    /// `exists(p)` and the place's presence fact key: a block that diverges under this
    /// guard proves the continuation.
    pub(super) fn negative_exists_guard<'e>(
        &self,
        cond: &'e Expression,
    ) -> Option<(&'e Expression, (&'a Family, Vec<u16>))> {
        let Expression::Unary {
            op: UnaryOp::Not,
            operand,
            ..
        } = cond
        else {
            return None;
        };
        self.exists_guard_fact(operand)
            .map(|fact| (&**operand, fact))
    }
}
