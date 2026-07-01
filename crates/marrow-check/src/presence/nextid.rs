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
//! The analysis is intentionally conservative: a call that may write a saved record
//! (an `append`, or a user function whose effect closure reaches a record write)
//! advances the cohorts of exactly the stores it writes — the per-store write identity
//! from its effect closure — so an unmodeled write to one store cannot suppress a
//! collision on a store it never touched. A call that merely allocates (a helper
//! returning `nextId`) writes no record, so it is the allocation it names and advances
//! no cohort.

use std::collections::HashMap;
use std::path::Path;

use super::keys::saved_place_key;
use super::scope::NameScope;
use crate::executable::accepted_saved_place;
use crate::facts::StoreId;
use crate::{
    CheckDiagnostic, CheckedBody, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr,
    CheckedFunctionRef, CheckedInterpolationPart, CheckedProgram, CheckedStmt,
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
            CheckedStmt::Assign { target, value, .. }
            | CheckedStmt::CompoundAssign { target, value, .. } => {
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
            CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => {}
        }
    }

    fn record_allocation(&mut self, binding: u32, value: &CheckedExpr) {
        let Some(store) = self.allocation_store(value) else {
            return;
        };
        // Seed the generation entry on first allocation so a later write to this store
        // (including one buried in a user function's effect closure) can advance its
        // cohort even before it has taken a direct write here.
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

    /// Walk an expression for record writes nested in value position — an `append`, or a
    /// user function whose effect closure reaches a record write.
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
                for store in super::writes::call_written_stores(self.program, target) {
                    self.advance(store);
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
            | CheckedExpr::SavedRoot { .. }
            | CheckedExpr::Absent { .. } => {}
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

impl Guard<'_> {
    /// The store an initializer allocates a fresh id from, if any. A direct
    /// `nextId(^store)` is the literal allocation; calling a helper whose return value
    /// originates from `nextId(^store)` is the same allocation one call deep, so the
    /// idiomatic allocator wrapper participates in the cohort the same way. A constructed
    /// identity (`Id(^store, key)`) names an existing id, not a fresh one, so it is never
    /// an allocation.
    fn allocation_store(&self, value: &CheckedExpr) -> Option<StoreId> {
        self.allocation_origin_store(value, &LocalBindings::empty(), &mut Vec::new())
    }

    /// The store a function allocates from when every value it returns originates from a
    /// `nextId` of that one store (directly, or through another allocator it calls). A
    /// function with no such return, or one whose returns name more than one store, is not
    /// a single-store allocator. The visited set bounds recursion through allocator chains
    /// and breaks cycles.
    fn allocator_function_store(
        &self,
        function_ref: CheckedFunctionRef,
        visited: &mut Vec<CheckedFunctionRef>,
    ) -> Option<StoreId> {
        if visited.contains(&function_ref) {
            return None;
        }
        visited.push(function_ref);
        let function = self
            .program
            .modules
            .get(function_ref.module as usize)?
            .functions
            .get(function_ref.function as usize)?;
        let body = function.runtime_body()?;
        let bindings = LocalBindings::from_body(body);
        let mut store = None;
        for value in returned_values(body) {
            let returned = self.allocation_origin_store(value, &bindings, visited)?;
            match store {
                None => store = Some(returned),
                Some(existing) if existing == returned => {}
                Some(_) => return None,
            }
        }
        store
    }

    /// The store an expression allocates from: a direct `nextId(^store)`, a call to an
    /// allocator function, or a single-segment name whose immutable local initializer
    /// originates from one of those. Following the name lets the idiomatic
    /// `const n = nextId(^s); return n` shape be recognized as an allocation rather than
    /// a constructed identity. Only immutable bindings are followed (a reassigned `var`
    /// is dropped before this runs), so a returned name traces to an allocation only when
    /// its value is unconditionally a fresh one.
    fn allocation_origin_store(
        &self,
        value: &CheckedExpr,
        bindings: &LocalBindings<'_>,
        visited: &mut Vec<CheckedFunctionRef>,
    ) -> Option<StoreId> {
        match value {
            CheckedExpr::Call { target, args, .. } => match target {
                CheckedCallTarget::Builtin(CheckedBuiltinCall::NextId) => {
                    Some(args.first()?.value.saved_place()?.store_id)
                }
                CheckedCallTarget::Function(function_ref) => {
                    self.allocator_function_store(*function_ref, visited)
                }
                _ => None,
            },
            CheckedExpr::Name { segments, .. } => {
                let [name] = segments.as_slice() else {
                    return None;
                };
                let initializer = bindings.follow(name)?;
                self.allocation_origin_store(initializer, &bindings.without(name), visited)
            }
            _ => None,
        }
    }
}

/// The initializer expression of every single-segment *immutable* local binding in a
/// function body, so a returned name can be traced back to the expression that produced
/// it. A reassigned `var` is excluded: its initializer no longer describes its value.
struct LocalBindings<'a> {
    initializers: HashMap<&'a str, &'a CheckedExpr>,
}

impl<'a> LocalBindings<'a> {
    fn empty() -> Self {
        Self {
            initializers: HashMap::new(),
        }
    }

    fn from_body(body: &'a CheckedBody) -> Self {
        let mut initializers = HashMap::new();
        collect_local_initializers(body, &mut initializers);
        Self { initializers }
    }

    fn follow(&self, name: &str) -> Option<&'a CheckedExpr> {
        self.initializers.get(name).copied()
    }

    /// The same bindings with one name dropped, so following that name's initializer
    /// cannot loop back through a self-reference.
    fn without(&self, name: &str) -> Self {
        let mut initializers = self.initializers.clone();
        initializers.remove(name);
        Self { initializers }
    }
}

/// Collect the immutable initializers a returned name may be traced through. A
/// `const` binding is immutable, so its initializer always describes the bound
/// value. A `var` may be reassigned after its initializer (on any path), in which
/// case the initializer no longer describes the returned value, so it is dropped:
/// classifying such a helper as an allocator on its initializer alone would warn on
/// safe code. A reassigned `var` is therefore conservatively not followed, leaving a
/// false negative for the rare `var` reassigned from a constructed id to `nextId`,
/// which is acceptable for a safety-net warning and never produces a false positive.
fn collect_local_initializers<'a>(
    body: &'a CheckedBody,
    initializers: &mut HashMap<&'a str, &'a CheckedExpr>,
) {
    let mut reassigned = Vec::new();
    collect_reassigned_names(body, &mut reassigned);
    for statement in body.statements() {
        let (name, value) = match statement {
            CheckedStmt::Const { name, value, .. } => (name, value),
            CheckedStmt::Var {
                name,
                value: Some(value),
                ..
            } => (name, value),
            _ => continue,
        };
        if reassigned.contains(&name.as_str()) {
            continue;
        }
        initializers.insert(name.as_str(), value);
    }
}

/// Names assigned to as a single-segment local target anywhere in the body,
/// including nested blocks and branches, so a `var` reassigned on any path is not
/// followed as if its initializer were its final value.
fn collect_reassigned_names<'a>(body: &'a CheckedBody, names: &mut Vec<&'a str>) {
    for statement in body.statements() {
        match statement {
            CheckedStmt::Assign { target, .. } => {
                if let CheckedExpr::Name { segments, .. } = target
                    && let [name] = segments.as_slice()
                {
                    names.push(name.as_str());
                }
            }
            CheckedStmt::If {
                then_block,
                else_ifs,
                else_block,
                ..
            }
            | CheckedStmt::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_reassigned_names(then_block, names);
                for else_if in else_ifs {
                    collect_reassigned_names(&else_if.block, names);
                }
                if let Some(else_block) = else_block {
                    collect_reassigned_names(else_block, names);
                }
            }
            CheckedStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_reassigned_names(&arm.block, names);
                }
            }
            CheckedStmt::While { body, .. }
            | CheckedStmt::For { body, .. }
            | CheckedStmt::Transaction { body, .. } => collect_reassigned_names(body, names),
            CheckedStmt::Try { body, catch, .. } => {
                collect_reassigned_names(body, names);
                if let Some(catch) = catch {
                    collect_reassigned_names(&catch.block, names);
                }
            }
            _ => {}
        }
    }
}

/// The value of every `return` statement reachable in a body, including those nested in
/// blocks, branches, loops, and transactions. A bare `return` with no value is not an
/// allocation site and is skipped, so a function that takes any value-free return path is
/// simply not classified as an allocator.
fn returned_values(body: &CheckedBody) -> Vec<&CheckedExpr> {
    let mut values = Vec::new();
    collect_returned_values(body, &mut values);
    values
}

fn collect_returned_values<'a>(body: &'a CheckedBody, values: &mut Vec<&'a CheckedExpr>) {
    for statement in body.statements() {
        match statement {
            CheckedStmt::Return {
                value: Some(value), ..
            } => values.push(value),
            CheckedStmt::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_returned_values(then_block, values);
                for else_if in else_ifs {
                    collect_returned_values(&else_if.block, values);
                }
                if let Some(else_block) = else_block {
                    collect_returned_values(else_block, values);
                }
            }
            CheckedStmt::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_returned_values(then_block, values);
                for else_if in else_ifs {
                    collect_returned_values(&else_if.block, values);
                }
                if let Some(else_block) = else_block {
                    collect_returned_values(else_block, values);
                }
            }
            CheckedStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_returned_values(&arm.block, values);
                }
            }
            CheckedStmt::While { body, .. }
            | CheckedStmt::For { body, .. }
            | CheckedStmt::Transaction { body, .. } => collect_returned_values(body, values),
            CheckedStmt::Try { body, catch, .. } => {
                collect_returned_values(body, values);
                if let Some(catch) = catch {
                    collect_returned_values(&catch.block, values);
                }
            }
            _ => {}
        }
    }
}
