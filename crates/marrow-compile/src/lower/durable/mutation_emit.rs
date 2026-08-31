use super::*;

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Lower a group-leaf read-modify-write `^root(k).group.leaf = value` or
    /// `delete ^root(k).group.leaf`: evaluate the key-path once into slots, read the whole
    /// group, and — only when the entry (and so the group) is present — rewrite the leaf
    /// slot (set present, or unset to vacant) on the materialized group record and replace
    /// the whole group. An absent entry short-circuits to a no-op: a group is a value unit
    /// of an existing entry, never created on its own. The group is materialized whole and
    /// written back, so a sibling leaf is preserved.
    pub(super) fn lower_group_leaf_rmw(
        &mut self,
        keys: &[DurKey],
        handle: &OccurrenceSiteHandle,
        slot: u16,
        edit: GroupLeafEdit,
        span: SourceSpan,
    ) -> ConstructResult<()> {
        let entry_site = self
            .site_operand(handle)
            .ok_or(LoweringFailure::Recoverable)?;
        // Evaluate each key column once into a fresh slot (root-first) so the read and the
        // replace key off the same evaluated columns. A group is a root-level value unit, so
        // its key-path is the root's — an identity operand spreads into the root's columns.
        let key_slots = self.capture_key_slots(keys, span)?;
        // A set evaluates its bare leaf value once into a slot before the read, so the read
        // record is on top of the stack when the leaf op runs.
        let value_slot = match &edit {
            GroupLeafEdit::Set { value, ty } => {
                let value_slot = self.alloc_slot(span).ok_or(LoweringFailure::Recoverable)?;
                self.lower_as(value, garg_to_lty(*ty))?;
                self.push(Instr::LocalSet(value_slot), span)?;
                Some(value_slot)
            }
            GroupLeafEdit::Unset => None,
        };
        // Read the group; present -> its materialized record is on the stack and the write
        // back runs; absent -> jump past the write back, a clean no-op (the group was never
        // there to modify).
        self.emit_slots(&key_slots, span)?;
        self.push(Instr::DurReadGroup(entry_site.clone()), span)?;
        let to_end = self.push_branch_present(span)?;
        // Present: rewrite the leaf slot on the materialized record, then replace the group.
        match edit {
            GroupLeafEdit::Set { .. } => {
                #[allow(
                    clippy::expect_used,
                    reason = "lowering bookkeeping: a `Set` edit lowers its value expression before this emit, so its result slot is bound"
                )]
                self.push(
                    Instr::LocalGet(value_slot.expect("a set evaluates its value")),
                    span,
                )?;
                self.push(Instr::FieldSet(slot), span)?;
            }
            GroupLeafEdit::Unset => {
                self.push(Instr::FieldUnset(slot), span)?;
            }
        }
        let rec_slot = self.alloc_slot(span).ok_or(LoweringFailure::Recoverable)?;
        self.push(Instr::LocalSet(rec_slot), span)?;
        self.emit_slots(&key_slots, span)?;
        self.push(Instr::LocalGet(rec_slot), span)?;
        self.push(Instr::DurReplaceGroup(entry_site), span)?;
        let end = self.here();
        self.patch(to_end, end);
        Ok(())
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
