//! The durable store handle and its read/transaction sessions (design §G).

use super::{
    AuthorizedSite, BoundedKeys, BoundedLimit, CommitResult, CreateOutcome, EntryValue,
    EraseOutcome, KernelFault, Presence, ReplaceOutcome,
};
use crate::codec::key::KeyScalar;
use crate::equality::ValueDomain;

mod address;
mod handle;
mod index_ops;
mod read_ops;
mod read_session;
mod resolve;
mod traverse;
mod txn_session;

pub use handle::DurableStore;
pub use read_session::ReadSession;
pub use txn_session::TxnSession;

/// The durable operations the VM drives. Object-safe so the VM holds a
/// `&mut dyn Durable` without knowing the concrete engine or session kind. A
/// read-only export drives a [`ReadSession`]; a mutating export drives a
/// [`TxnSession`]. The verifier guarantees a read-only session never reaches a
/// mutation.
pub trait Durable {
    /// The authorized site at image site index `index`.
    fn site(&self, index: u16) -> AuthorizedSite;
    /// Every node-addressing op takes the addressed node's key-path: `[root_key]` for
    /// a root node and `[root_key, branch_key, …]` for a branch node, matching the
    /// site's root and branch-hop arity. A root site's key-path is one element.
    fn presence(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault>;
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<ValueDomain>, KernelFault>;
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault>;
    /// Materialize the record of one unkeyed group of the entry `keys` addresses: one
    /// slot per group field, present or vacant. A group's presence is its containing
    /// entry's presence, so this yields `None` exactly when the entry is payload-absent
    /// and otherwise the group's leaves (a group with all-vacant leaves reads present
    /// with every slot vacant). It reads only the group's own leaves — never the entry's
    /// top-level fields, a sibling group, or a branch.
    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault>;
    /// Exact replacement of one group of the entry `keys` addresses, scoped to the
    /// group's own field set: remove every one of the group's leaves, then write the
    /// leaf for each present field of `value`. Omitted sparse leaves do not survive
    /// (replace, not merge). A group has no independent existence, so a replace over a
    /// payload-absent entry is [`ReplaceOutcome::Missing`] and touches nothing; over a
    /// present entry the entry marker, its top-level fields, its sibling groups, and its
    /// branches are all left intact (the group-scoped payload-only law).
    fn replace_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault>;
    /// Erase one group of the entry `keys` addresses: remove every one of the group's own
    /// leaves and nothing else. [`EraseOutcome::Erased`] when any leaf existed, else
    /// [`EraseOutcome::Missing`]. The entry marker, its top-level fields, its sibling
    /// groups, and its branches are preserved.
    fn erase_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    /// Freeze the first `limit` immediate keys of the layer the whole-entry `site`
    /// belongs to — the root's entry family (a `WholePayload` site) or a keyed branch
    /// family beneath a fixed parent (a branch site) — starting at an inclusive `from`
    /// key when given, and report whether a further key existed. `ancestor_keys` is the
    /// key-path to the traversed layer's parent: empty for the root layer, `[root_key]`
    /// for a single-level branch layer — one fewer than the site's whole-entry key
    /// arity, since the traversed key is what iteration enumerates rather than an
    /// operand. At most `limit + 1` distinct present keys are acquired and the frozen
    /// set is bounded by `limit`. The walk costs `O(limit + 1 + d)` seeks, where `d` is
    /// the number of descendant-only siblings interspersed among the visited keys: a
    /// descendant-only child (branch children, no payload) is skipped by one
    /// prefix-successor seek past its subtree, and its own fan-out — however large — is
    /// never read.
    fn iterate_bounded(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault>;
    /// Freeze the first `limit` distinct values of a nonunique managed index's next
    /// projected component, holding the leading components `prefix` (a strict prefix of
    /// the index's ordered projection), starting at an inclusive `from` component when
    /// given, and report whether a further distinct value existed. Like
    /// [`Self::iterate_bounded`] this is a bounded progressive refinement: it acquires at
    /// most `limit + 1` distinct component values through the index cell family, costs
    /// `O(limit + 1)` seeks regardless of how many rows share each value (one
    /// prefix-successor seek passes a whole value's rows), and establishes no presence
    /// fact — an index scan observes only the derived index, never a source entry.
    fn index_scan(
        &mut self,
        site: &AuthorizedSite,
        prefix: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault>;
    /// Look up the single source key tuple a unique managed index maps the complete
    /// projection `key` to, or [`None`] when no row matches. One exact probe of the index
    /// cell family; it yields exactly the matching source key or absent, never a sibling,
    /// and observes no source entry.
    fn index_lookup(
        &mut self,
        site: &AuthorizedSite,
        key: &[KeyScalar],
    ) -> Result<Option<Vec<KeyScalar>>, KernelFault>;
    /// Whether the layer the whole-entry `site` names — the root's entry family (a root
    /// site) or a keyed branch family beneath the parent entry `ancestor_keys` locates (a
    /// branch site) — has at least one payload-bearing immediate child. One forward
    /// `layer_step` from the layer's start: a present child yields `Present`, an empty
    /// or purely descendant-only layer yields `Absent`. Descendant-only children (branch
    /// children with no payload marker) are skipped by one prefix-successor seek each, so
    /// the probe reads at most one payload child key and observes no values. Like the
    /// bounded traversal it establishes no per-key presence fact; it answers only the
    /// family-populated question.
    fn family_populated(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault>;
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: ValueDomain,
    ) -> Result<(), KernelFault>;
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault>;
    /// Set (present) or clear (vacant) a sparse field of an entry the caller has
    /// statically proven present. Asserts the entry marker is present — a violation
    /// is a marker/field mismatch ([`KernelFault::Corruption`]), never implicit
    /// creation — then stages the leaf exactly like [`Self::set_sparse`].
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault>;
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault>;
    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault>;
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    fn erase_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    /// Commit the transaction (a no-op returning [`CommitResult::Committed`] for a
    /// read-only session, which the verifier guarantees never opens one).
    fn commit(&mut self) -> CommitResult;
}

#[cfg(test)]
mod tests;
