use super::*;

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Lower `p.group.leaf = value` on an entry a presence proof covers: evaluate the
    /// leaf value once, read the whole group through the place's key slots (a bare
    /// record — the entry is present), rewrite the leaf slot, and replace the whole
    /// group so a sibling leaf is preserved.
    pub(super) fn lower_group_leaf_set(
        &mut self,
        key_slots: &[u16],
        handle: &OccurrenceSiteHandle,
        slot: u16,
        value: &Expression,
        ty: GArg,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let site = self
            .site_operand(handle)
            .ok_or(LoweringFailure::Recoverable)?;
        let value_slot = self.alloc_slot(span).ok_or(LoweringFailure::Recoverable)?;
        self.lower_as(value, garg_to_lty(ty))?;
        self.push(Instr::LocalSet(value_slot), span)?;
        self.push(
            Instr::DurReadGroupPresent {
                site: site.clone(),
                key_slots: key_slots.to_vec(),
            },
            span,
        )?;
        self.push(Instr::LocalGet(value_slot), span)?;
        self.push(Instr::FieldSet(slot), span)?;
        self.replace_group_from_stack(key_slots, site, span)
    }

    /// Lower `delete p.group.leaf` / `delete ^root(k).group.leaf`: a delete needs no
    /// proof, so the group is read through the optional form and only a present entry
    /// has its leaf cleared and the group replaced; an absent entry is a no-op.
    pub(super) fn lower_group_leaf_unset(
        &mut self,
        keys: &[DurKey],
        handle: &OccurrenceSiteHandle,
        slot: u16,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let site = self
            .site_operand(handle)
            .ok_or(LoweringFailure::Recoverable)?;
        let key_slots = self.capture_key_slots(keys, span)?;
        self.emit_slots(&key_slots, span)?;
        self.push(Instr::DurReadGroup(site.clone()), span)?;
        let to_end = self.push_branch_present(span)?;
        self.push(Instr::FieldUnset(slot), span)?;
        self.replace_group_from_stack(&key_slots, site, span)?;
        let end = self.here();
        self.patch(to_end, end);
        Ok(())
    }

    /// Replace the group at `site` with the rewritten record on top of the stack, keyed
    /// by the containing entry's `key_slots`.
    fn replace_group_from_stack(
        &mut self,
        key_slots: &[u16],
        site: PlannedSiteRef,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let rec_slot = self.alloc_slot(span).ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::LocalSet(rec_slot), span)?;
        self.emit_slots(key_slots, span)?;
        self.push(Instr::LocalGet(rec_slot), span)?;
        self.push(Instr::DurReplaceGroup(site), span)
    }

    /// Lower `^r(k) = record` or `^r(k).branch(bk) = Resource.branch(...)` to the
    /// transaction-local presence branch (design §D): `DurExists` over the entry's whole
    /// key-path decides `replace` vs `create` against the coherent staged view. The
    /// key-path is materialized into slots (one per column, root first) so the exists,
    /// replace, and create ops all key off the same evaluated columns.
    pub(super) fn lower_upsert(
        &mut self,
        keys: &[DurKey],
        handle: &OccurrenceSiteHandle,
        record: TypeId,
        value: &Expression,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let entry_site = self
            .site_operand(handle)
            .ok_or(LoweringFailure::Recoverable)?;
        // A bound (place) column already holds its key in a pre-evaluated slot; reuse it
        // so the create/replace ops key off it (the verifier's presence lattice
        // recognizes a root create as establishing that slot's entry). An inline column
        // is evaluated once into a fresh slot. An entry-identity root column spreads into
        // the root's key columns, so the exists/replace/create ops key off the same
        // evaluation whether the whole-entry address is a root (identity or per-column) or
        // a branch below an identity-keyed root.
        let key_slots: Vec<u16> = self.capture_key_slots(keys, span)?;
        let rec_slot = self.alloc_slot(span).ok_or(LoweringFailure::Recoverable)?;
        self.lower_as(
            value,
            LTy::Record {
                ty: record,
                optional: false,
            },
        )?;
        self.push(Instr::LocalSet(rec_slot), span)?;

        self.emit_slots(&key_slots, span)?;
        self.push(Instr::DurExists(entry_site.clone()), span)?;
        let to_create = self.push_jif(span)?;
        // Present: replace.
        self.emit_slots(&key_slots, span)?;
        self.push(Instr::LocalGet(rec_slot), span)?;
        self.push(Instr::DurReplaceEntry(entry_site.clone()), span)?;
        let to_end = self.push_jump(span)?;
        // Absent: create.
        let create_at = self.here();
        self.patch(to_create, create_at);
        self.emit_slots(&key_slots, span)?;
        self.push(Instr::LocalGet(rec_slot), span)?;
        self.push(Instr::DurCreateEntry(entry_site), span)?;
        let end = self.here();
        self.patch(to_end, end);
        Ok(())
    }

    /// Push a durable operation's key-path from pre-evaluated slots, root column first,
    /// so the innermost key lands on top — the order the kernel's `pop_key_path` reads.
    fn emit_slots(&mut self, slots: &[u16], span: SourceSpan) -> ConstructResult<()> {
        for slot in slots {
            self.push(Instr::LocalGet(*slot), span)?;
        }
        Ok(())
    }
}
