//! Presence proofs over durable entries: the facts a guard or a whole-entry write
//! establishes, the loop regions and calls that end them, and the refusal of a
//! presence-dependent use no fact covers.

use super::*;

/// A loop's patch targets — where `continue` jumps, and the jumps `break` emits that
/// must be patched to the loop's exit once it is known — and its proof region: a loop
/// body is one region, so a use through a fact older than the loop is refused when
/// the body erases the fact's family or calls a function that erases it, because the
/// back edge puts that erase before the use.
pub(super) struct LoopCtx<'a> {
    pub(super) continue_target: usize,
    pub(super) break_jumps: Vec<usize>,
    /// Families an entry erase inside this body (or a nested body) ended.
    pub(super) erased_families: Vec<&'a Family>,
    /// Start of the repeating region in the function's ordered call log.
    pub(super) call_start: usize,
    /// Protected uses inside this body through facts established before it was
    /// entered: the use's span and its family, resolved when the loop closes.
    pub(super) obligations: Vec<(SourceSpan, &'a Family)>,
}

impl<'a> LoopCtx<'a> {
    pub(super) fn new(continue_target: usize, call_start: usize) -> Self {
        Self {
            continue_target,
            break_jumps: Vec::new(),
            erased_families: Vec::new(),
            call_start,
            obligations: Vec::new(),
        }
    }
}

/// A presence proof over one durable entry: the compiler knows the entry addressed by
/// `key_slots` (its whole key-path as pre-evaluated place slots, root-first) in
/// `family` is present from here to the end of the block that established the fact,
/// unless an erase of the family or a call that erases it ends it first. Retain
/// invalidated identity until lexical exit so a required read cannot silently
/// revert to an untested optional read after losing its proof.
pub(super) struct PresenceFact<'a> {
    pub(super) family: &'a Family,
    pub(super) key_slots: Vec<u16>,
    pub(super) state: PresenceState,
}

pub(super) enum PresenceState {
    /// Loop nesting and the start of the single call log at establishment. Uses
    /// record at most one crossed-loop interval in addition to their ordinary one.
    Live {
        depth: usize,
        call_start: usize,
    },
    Invalidated,
}

/// A protected use whose proof a call may have ended: the use's span, its
/// family, and an interval in its function's ordered call log. A second interval
/// may cover the outermost repeating region entered after the proof.
pub(crate) struct PresenceObligation {
    pub span: SourceSpan,
    pub family: Family,
    pub calls: std::ops::Range<usize>,
}

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Record that the entry `key_slots` addresses in `family` is known present from
    /// here (a dominating guard or a completed upsert) until the enclosing block ends,
    /// the family is erased, or a call erases it. A fact re-established inside a loop
    /// is a fresh fact at the loop's depth, so a per-iteration guard is never charged
    /// with an outer fact's obligations.
    pub(super) fn mark_present(&mut self, family: &'a Family, key_slots: Vec<u16>) {
        self.present_places.push(PresenceFact {
            family,
            key_slots,
            state: PresenceState::Live {
                depth: self.loops.len(),
                call_start: self.calls.len(),
            },
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

    /// Select the newest lexical fact before checking validity. An invalidated
    /// checked view is a refusal; only a place never checked in scope is untested.
    /// Record calls and crossed-loop obligations once for a live selected fact.
    pub(super) fn checked_key_slots(
        &mut self,
        family: &'a Family,
        key_slots: Option<Vec<u16>>,
        span: SourceSpan,
    ) -> ConstructResult<Option<Vec<u16>>> {
        let Some((key_slots, fact)) =
            key_slots.and_then(|slots| self.present_fact(family, &slots).map(|fact| (slots, fact)))
        else {
            return Ok(None);
        };
        let PresenceState::Live { depth, call_start } = fact.state else {
            self.fail(requires_presence(
                self.file,
                span,
                "the entry's presence proof was invalidated",
            ));
            return Err(LoweringFailure::Recoverable);
        };
        let calls = call_start..self.calls.len();
        if !calls.is_empty() {
            self.presence_obligations.push(PresenceObligation {
                span,
                family: family.clone(),
                calls,
            });
        }
        if let Some(ctx) = self.loops.get_mut(depth) {
            ctx.obligations.push((span, family));
        }
        Ok(Some(key_slots))
    }

    /// A mutation requires a checked view even when its address is untested.
    pub(super) fn require_present(
        &mut self,
        family: &'a Family,
        key_slots: Option<Vec<u16>>,
        span: SourceSpan,
    ) -> ConstructResult<Vec<u16>> {
        if let Some(slots) = self.checked_key_slots(family, key_slots, span)? {
            return Ok(slots);
        }
        self.fail(requires_presence(
            self.file,
            span,
            "no presence proof covers the entry here",
        ));
        Err(LoweringFailure::Recoverable)
    }

    /// End every proof over `family`: its entry payload may be gone. Every open loop
    /// records the erase, so a protected use earlier in its body through an older fact is
    /// refused when the loop closes.
    pub(super) fn erase_family(&mut self, family: &'a Family) {
        if !self.erased_families.contains(&family) {
            self.erased_families.push(family);
        }
        // Lexical scope marks are vector positions, so erasure must not move
        // surviving facts below a mark that will later truncate the scope.
        for fact in &mut self.present_places {
            if fact.family == family {
                fact.state = PresenceState::Invalidated;
            }
        }
        for ctx in &mut self.loops {
            if !ctx.erased_families.contains(&family) {
                ctx.erased_families.push(family);
            }
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
                    "the loop body erases an entry of the family after this use, so \
                     the next iteration uses an entry the proof no longer covers",
                ));
            } else if ctx.call_start < self.calls.len() {
                self.presence_obligations.push(PresenceObligation {
                    span,
                    family: family.clone(),
                    calls: ctx.call_start..self.calls.len(),
                });
            }
        }
        for fact in &mut self.present_places {
            if ctx.erased_families.contains(&fact.family) {
                fact.state = PresenceState::Invalidated;
            }
        }
        ctx.break_jumps
    }

    /// If `cond` is `exists(p)` over an in-scope named `place`, that place's presence
    /// fact key: the guarded (then) block may use the checked place.
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
