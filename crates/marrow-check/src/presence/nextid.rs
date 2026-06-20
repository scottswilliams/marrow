//! Duplicate-key warning for consecutive `nextId` allocations.
//!
//! `nextId(^s)` returns the next-available identity (`max + 1`) but does not
//! advance the allocation until a record is actually written. So two `nextId(^s)`
//! calls with no write to `^s` between them return the *same* value. Binding both
//! and writing each as its own record key inserts the same record twice — the
//! second write silently overwrites the first.
//!
//! This pass walks each body in source order and flags that footgun. Two `nextId`
//! bindings for one store belong to the same *cohort* when no write to that store
//! occurred between their allocations; a write to a store advances its cohort, so
//! the safe interleaved form (`allocate, write, allocate, write`) places the two
//! allocations in different cohorts. The warning fires only when two bindings of
//! the same cohort are both written as record keys on one execution path.
//!
//! Branch arms run on disjoint paths, so two writes in mutually-exclusive arms of
//! one `if`/`match`/`try` never both commit and cannot overwrite each other. Each
//! arm walks from a snapshot of the cohort state and the arms join afterward (a
//! cohort advances to the per-arm maximum, and a binding written on any arm is
//! conservatively recorded as used), so a write in one arm is never seen as a
//! colliding sibling of a write in another.
//!
//! The analysis is intentionally conservative: a call that may write saved data
//! (an `append`, or a user function whose effect closure writes) advances every
//! live cohort, so an unmodeled write can only suppress the warning, never invent
//! one.

use std::collections::HashMap;
use std::path::Path;

use super::keys::saved_place_key;
use super::scope::NameScope;
use crate::executable::accepted_saved_place;
use crate::facts::StoreId;
use crate::{
    CheckDiagnostic, CheckedBody, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr,
    CheckedInterpolationPart, CheckedProgram, CheckedStmt,
};

pub(super) fn check_next_id_collisions(
    program: &CheckedProgram,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for module in &program.modules {
        for function in &module.functions {
            let Some(body) = function.runtime_body() else {
                continue;
            };
            let mut scope = NameScope::for_function(function, &module.source_file);
            let mut guard = Guard::new(program, diagnostics);
            guard.walk_block(body, &mut scope);
        }
    }
    for transform in &program.catalog.evolve_transforms {
        let Some(body) = transform.runtime_body() else {
            continue;
        };
        let mut scope = NameScope::for_transform(&transform.resource);
        let mut guard = Guard::new(program, diagnostics);
        guard.walk_block(body, &mut scope);
    }
}

/// One `const`/`var` whose initializer was `nextId(^store)`, with the cohort
/// generation it was allocated in and whether it has since been written as a key.
#[derive(Clone)]
struct Allocation {
    binding: u32,
    store: StoreId,
    generation: u32,
    used: bool,
}

/// The cohort bookkeeping that branch arms must not share. Two arms of one
/// branch run on disjoint paths, so a write in one arm cannot collide with a
/// write in a sibling arm; each arm walks from a snapshot of this state and the
/// snapshots join afterward (per-store max generation, used-on-any-path).
#[derive(Clone)]
struct CohortState {
    allocations: Vec<Allocation>,
    generations: HashMap<StoreId, u32>,
}

struct Guard<'a> {
    program: &'a CheckedProgram,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
    cohorts: CohortState,
}

impl<'a> Guard<'a> {
    fn new(program: &'a CheckedProgram, diagnostics: &'a mut Vec<CheckDiagnostic>) -> Self {
        Self {
            program,
            diagnostics,
            cohorts: CohortState {
                allocations: Vec::new(),
                generations: HashMap::new(),
            },
        }
    }

    /// A write to `store` advances its cohort: identities allocated afterward are
    /// distinct from those allocated before, so they never collide.
    fn advance(&mut self, store: StoreId) {
        let generation = self.cohorts.generations.entry(store).or_insert(0);
        *generation += 1;
    }

    /// Any write of unknown destination conservatively advances the cohort of every
    /// store with a live allocation, so an unmodeled write suppresses rather than
    /// invents a warning. Stores are seeded into the generation map when they first
    /// allocate, so a whole-cohort advance reaches even cohorts that have not yet
    /// taken a direct write.
    fn advance_all(&mut self) {
        for generation in self.cohorts.generations.values_mut() {
            *generation += 1;
        }
    }

    /// Walk each block of a branch from a shared snapshot so sibling arms never see
    /// each other's writes, then join their cohort states. The generation joins to
    /// the per-store maximum and a binding written on any arm is conservatively
    /// `used`; a collision is only flagged when two siblings are written on one path.
    fn walk_branch_arms(
        &mut self,
        arms: &mut dyn Iterator<Item = &CheckedBody>,
        scope: &mut NameScope,
    ) {
        let entry = self.cohorts.clone();
        let mut joined: Option<CohortState> = None;
        for arm in arms {
            self.cohorts = entry.clone();
            self.walk_block(arm, scope);
            joined = Some(match joined.take() {
                None => self.cohorts.clone(),
                Some(acc) => join_cohorts(acc, &self.cohorts),
            });
        }
        self.cohorts = joined.unwrap_or(entry);
    }

    fn walk_block(&mut self, block: &CheckedBody, scope: &mut NameScope) {
        scope.push_frame();
        for statement in block.statements() {
            self.walk_statement(statement, scope);
        }
        scope.pop_frame();
    }

    fn walk_statement(&mut self, statement: &CheckedStmt, scope: &mut NameScope) {
        match statement {
            CheckedStmt::Const {
                name,
                binding_type,
                value,
                ..
            } => {
                self.observe_writes(value, scope);
                let binding = scope.bind_with_type(name, binding_type.clone());
                self.record_allocation(binding, value);
            }
            CheckedStmt::Var {
                name,
                binding_type,
                value,
                ..
            } => {
                if let Some(value) = value {
                    self.observe_writes(value, scope);
                }
                let binding = scope.bind_with_type(name, binding_type.clone());
                if let Some(value) = value {
                    self.record_allocation(binding, value);
                }
            }
            CheckedStmt::Assign { target, value, .. } => {
                self.observe_writes(value, scope);
                self.observe_saved_write(target, scope);
            }
            CheckedStmt::Delete { path, .. } => {
                self.observe_saved_write(path, scope);
            }
            CheckedStmt::Throw { value, .. } | CheckedStmt::Expr { value, .. } => {
                self.observe_writes(value, scope);
            }
            CheckedStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.observe_writes(value, scope);
                }
            }
            CheckedStmt::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(condition) = condition {
                    self.observe_writes(condition, scope);
                }
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        self.observe_writes(condition, scope);
                    }
                }
                let mut arms = std::iter::once(then_block)
                    .chain(else_ifs.iter().map(|else_if| &else_if.block))
                    .chain(else_block.as_ref());
                self.walk_branch_arms(&mut arms, scope);
            }
            CheckedStmt::IfConst {
                name,
                binding_type,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.observe_writes(value, scope);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        self.observe_writes(condition, scope);
                    }
                }
                scope.push_frame();
                let binding = scope.bind_with_type(name, binding_type.clone());
                self.record_allocation(binding, value);
                let mut arms = std::iter::once(then_block)
                    .chain(else_ifs.iter().map(|else_if| &else_if.block))
                    .chain(else_block.as_ref());
                self.walk_branch_arms(&mut arms, scope);
                scope.pop_frame();
            }
            CheckedStmt::While {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.observe_writes(condition, scope);
                }
                self.walk_block(body, scope);
            }
            CheckedStmt::For {
                binding,
                iterable,
                step,
                body,
                ..
            } => {
                self.observe_writes(iterable, scope);
                if let Some(step) = step {
                    self.observe_writes(step, scope);
                }
                scope.push_frame();
                scope.bind(&binding.first);
                if let Some(second) = &binding.second {
                    scope.bind(second);
                }
                self.walk_block(body, scope);
                scope.pop_frame();
            }
            CheckedStmt::Transaction { body, .. } => self.walk_block(body, scope),
            CheckedStmt::Try { body, catch, .. } => {
                // `catch` runs only when `body` aborts, so a write in `body` and one
                // in `catch` are never both committed on one path; walk them as
                // disjoint arms rather than back to back on shared state.
                let entry = self.cohorts.clone();
                self.walk_block(body, scope);
                if let Some(catch) = catch {
                    let after_body = std::mem::replace(&mut self.cohorts, entry);
                    scope.push_frame();
                    scope.bind(&catch.name);
                    self.walk_block(&catch.block, scope);
                    scope.pop_frame();
                    self.cohorts = join_cohorts(after_body, &self.cohorts);
                }
            }
            CheckedStmt::Match {
                scrutinee, arms, ..
            } => {
                if let Some(scrutinee) = scrutinee {
                    self.observe_writes(scrutinee, scope);
                }
                let mut blocks = arms.iter().map(|arm| &arm.block);
                self.walk_branch_arms(&mut blocks, scope);
            }
            CheckedStmt::ReturnAbsent { .. }
            | CheckedStmt::Break { .. }
            | CheckedStmt::Continue { .. } => {}
        }
    }

    fn record_allocation(&mut self, binding: u32, value: &CheckedExpr) {
        let Some(store) = next_id_store(value) else {
            return;
        };
        // Seed the generation entry on first allocation so a whole-cohort advance
        // (a user-function write of unknown destination) can reach this store even
        // before it has taken a direct write.
        let generation = *self.cohorts.generations.entry(store).or_insert(0);
        self.cohorts.allocations.push(Allocation {
            binding,
            store,
            generation,
            used: false,
        });
    }

    /// Record a write to a saved place: flag a cohort collision if its key reuses a
    /// sibling allocation already written, then advance the store's cohort.
    fn observe_saved_write(&mut self, target: &CheckedExpr, scope: &mut NameScope) {
        self.observe_writes(target, scope);
        let Some(place) = accepted_saved_place(target) else {
            return;
        };
        let store = place.store_id;
        if let Some(key) = saved_place_key(target, scope) {
            let file = scope.source_file().to_path_buf();
            self.flag_key_bindings(store, &key.key_bindings, target, &file);
        }
        self.advance(store);
    }

    /// Mark each `nextId` allocation referenced by these write-key bindings as used.
    /// If a same-cohort sibling was already used, the two writes collide.
    fn flag_key_bindings(
        &mut self,
        store: StoreId,
        key_bindings: &[u32],
        target: &CheckedExpr,
        file: &Path,
    ) {
        for binding in key_bindings {
            let Some(index) = self.allocation_index(store, *binding) else {
                continue;
            };
            let generation = self.cohorts.allocations[index].generation;
            let collides = self
                .cohorts
                .allocations
                .iter()
                .enumerate()
                .any(|(other, alloc)| {
                    other != index
                        && alloc.used
                        && alloc.store == store
                        && alloc.generation == generation
                });
            self.cohorts.allocations[index].used = true;
            if collides {
                self.warn(target, file);
            }
        }
    }

    fn allocation_index(&self, store: StoreId, binding: u32) -> Option<usize> {
        self.cohorts
            .allocations
            .iter()
            .position(|alloc| alloc.store == store && alloc.binding == binding)
    }

    /// Walk an expression for writes nested in value position — an `append`, or a
    /// user function whose effect closure writes saved data.
    fn observe_writes(&mut self, expr: &CheckedExpr, scope: &mut NameScope) {
        match expr {
            CheckedExpr::Call {
                callee,
                args,
                target,
                ..
            } => {
                if let CheckedCallTarget::Builtin(CheckedBuiltinCall::Append) = target
                    && let Some((first, rest)) = args.split_first()
                {
                    for arg in rest {
                        self.observe_writes(&arg.value, scope);
                    }
                    self.observe_saved_write(&first.value, scope);
                    return;
                }
                self.observe_writes(callee, scope);
                for arg in args {
                    self.observe_writes(&arg.value, scope);
                }
                if super::writes::call_writes_saved_data(self.program, target) {
                    self.advance_all();
                }
            }
            CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
                self.observe_writes(base, scope);
            }
            CheckedExpr::Unary { operand, .. } => self.observe_writes(operand, scope),
            CheckedExpr::Binary { left, right, .. } => {
                self.observe_writes(left, scope);
                self.observe_writes(right, scope);
            }
            CheckedExpr::Range {
                start, end, step, ..
            } => {
                for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    self.observe_writes(part, scope);
                }
            }
            CheckedExpr::Interpolation { parts, .. } => {
                for part in parts {
                    if let CheckedInterpolationPart::Expr(expr) = part {
                        self.observe_writes(expr, scope);
                    }
                }
            }
            CheckedExpr::Literal { .. }
            | CheckedExpr::Name { .. }
            | CheckedExpr::SavedRoot { .. } => {}
        }
    }

    fn warn(&mut self, target: &CheckedExpr, file: &Path) {
        self.diagnostics.push(CheckDiagnostic::warning(
            crate::CHECK_NEXT_ID_COLLISION,
            file,
            target.span(),
            "two nextId values for the same store are written as distinct keys with no \
             write between the allocations, so they are equal and the second write overwrites \
             the first; write each record before allocating the next id",
        ));
    }
}

/// Join two arm cohort states: per-store max generation, and a binding is `used`
/// if it was written on either arm. Allocations are matched by their scope binding
/// id, so an arm-local allocation that the sibling never saw is carried through.
fn join_cohorts(mut acc: CohortState, other: &CohortState) -> CohortState {
    for store in other.generations.keys() {
        let other_generation = other.generations[store];
        acc.generations
            .entry(*store)
            .and_modify(|generation| *generation = (*generation).max(other_generation))
            .or_insert(other_generation);
    }
    for allocation in &other.allocations {
        match acc
            .allocations
            .iter_mut()
            .find(|existing| existing.binding == allocation.binding)
        {
            Some(existing) => existing.used |= allocation.used,
            None => acc.allocations.push(allocation.clone()),
        }
    }
    acc
}

fn next_id_store(value: &CheckedExpr) -> Option<StoreId> {
    let CheckedExpr::Call { target, args, .. } = value else {
        return None;
    };
    if !matches!(
        target,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::NextId)
    ) {
        return None;
    }
    Some(args.first()?.value.saved_place()?.store_id)
}
