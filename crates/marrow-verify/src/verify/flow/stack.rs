//! The abstract operand stack of one function's flow check, as interned prefixes.
//!
//! A stack is a frozen prefix plus a working suffix. A frozen prefix is a handle to a
//! node in one per-function arena, and a node is minted through a map keyed by
//! `(parent, cell)`, so equal prefixes have equal handles. A jump target retains its
//! incoming stack as one handle and a merge compares two handles; resuming a target
//! copies nothing. Only cells still live at a fork or boundary are interned, each once
//! per freeze, so flow verification costs the same at any stack-depth limit.

use super::work;
use crate::vtype::VType;
use std::collections::HashMap;

/// A handle to one interned operand-stack prefix. Equal handles are equal stacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct StackId(u32);

impl StackId {
    /// The empty stack, which has no node.
    pub(super) const EMPTY: Self = Self(0);
}

/// The stack `parent` with `cell` pushed on top, `depth` cells deep.
struct Node {
    parent: StackId,
    cell: VType,
    depth: u32,
}

/// One function's interned prefixes.
///
/// The map hashes with the default keyed `RandomState`: its keys are chosen by the
/// image, which is untrusted, and a fixed-key hasher would let a crafted image force
/// collisions. The map is only probed, never iterated, so its seed cannot reach a
/// verdict.
#[derive(Default)]
struct Arena {
    /// `nodes[i]` is the node of `StackId(i + 1)`.
    nodes: Vec<Node>,
    interned: HashMap<(StackId, VType), StackId>,
}

impl Arena {
    fn node(&self, id: StackId) -> Option<&Node> {
        let index = id.0.checked_sub(1)?;
        Some(&self.nodes[index as usize])
    }

    fn depth(&self, id: StackId) -> usize {
        self.node(id).map_or(0, |node| node.depth as usize)
    }

    /// The handle of `parent` with `cell` on top, minting its node on first use.
    ///
    /// A region re-run freezes exactly the cells its first run froze onto the same
    /// handles, so every node is minted by a first run. First runs execute each
    /// instruction once and one instruction pushes at most `MAX_KEY_COLUMNS` cells, so a
    /// function mints at most `MAX_KEY_COLUMNS x MAX_CODE_BYTES` nodes, far below the
    /// `u32` id space; `rerunning_cell_keeping_links_refinds_their_nodes` pins it.
    fn intern(&mut self, parent: StackId, cell: VType) -> StackId {
        let depth = self.node(parent).map_or(0, |node| node.depth) + 1;
        let nodes = &mut self.nodes;
        *self.interned.entry((parent, cell)).or_insert_with(|| {
            work::minted();
            nodes.push(Node {
                parent,
                cell,
                depth,
            });
            StackId(u32::try_from(nodes.len()).expect("stack nodes fit a u32 id"))
        })
    }
}

/// The working operand stack: the frozen prefix `base`, shared with retained states,
/// and the cells pushed on top of it since the last freeze.
pub(super) struct OperandStack {
    arena: Arena,
    base: StackId,
    suffix: Vec<VType>,
}

impl OperandStack {
    pub(super) fn new() -> Self {
        Self {
            arena: Arena::default(),
            base: StackId::EMPTY,
            suffix: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, cell: VType) {
        self.suffix.push(cell);
    }

    /// Pop the top cell, or `None` on the empty stack.
    pub(super) fn pop(&mut self) -> Option<VType> {
        if let Some(cell) = self.suffix.pop() {
            return Some(cell);
        }
        let node = self.arena.node(self.base)?;
        let cell = node.cell;
        self.base = node.parent;
        Some(cell)
    }

    /// The top cell, or `None` on the empty stack.
    pub(super) fn top(&self) -> Option<VType> {
        match self.suffix.last() {
            Some(cell) => Some(*cell),
            None => self.arena.node(self.base).map(|node| node.cell),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.suffix.is_empty() && self.base == StackId::EMPTY
    }

    pub(super) fn depth(&self) -> usize {
        self.arena.depth(self.base) + self.suffix.len()
    }

    /// Intern the suffix onto the frozen prefix and return the handle of the whole
    /// stack, for a jump target to retain.
    pub(super) fn freeze(&mut self) -> StackId {
        work::cells(self.suffix.len());
        for cell in self.suffix.drain(..) {
            self.base = self.arena.intern(self.base, cell);
        }
        self.base
    }

    /// Make the retained stack `id` the current stack, dropping any suffix a region
    /// ended without consuming.
    pub(super) fn resume(&mut self, id: StackId) {
        self.base = id;
        self.suffix.clear();
    }
}
