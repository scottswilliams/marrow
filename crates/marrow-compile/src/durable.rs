//! The durable graph registry (design §B/§C).
//!
//! The durable graph admits one or more `store` roots, each over its own resource
//! record and in declaration order (a root's DURABLE-table index is its RootId). A root
//! is a *singleton* (`store ^root: Record`, no key) or a
//! *keyed tuple* (`store ^root(k1: K1, k2: K2): Record`, one or more ordered
//! orderable durable-key columns). A resource's durable shape is a **member tree**:
//! its top-level stored fields, plus any static `group` field-path namespaces and
//! keyed `branch` placements, each of which recursively holds its own members. A
//! group is an unkeyed pathing construct (a `Group` ledger identity); a branch is a
//! keyed subtree — a distinct graph node with its own placement id and key tuple,
//! just like a root. Every admitted node has a complete ledger identity and a
//! contribution to the durable-contract identity the verifier independently
//! re-encodes.
//!
//! The executable durable subset the single-root kernel can serve at this stage is a flat
//! keyed root: one or more key columns, whose top-level fields are each a scalar or a
//! widened value (`struct`/`enum`/`Option`, framed inline), whose root-level `group`
//! members hold only such fields, and whose keyed `branch` placements are field-only
//! (nested to any depth). A singleton (keyless) root, a root whose resource declares a
//! nominal-typed field, a group nested in a branch or in another group, completes its
//! identity and verifies but has no executable operation sites — an operation over one is a
//! precise typed `check.unsupported` rejection at lowering ("not yet executable"). Those
//! shapes run when their lanes land. This module validates the declaration, adds the root,
//! its member tree, and — for the executable subset — its operation sites to the draft, and
//! exposes the resolved sites the function lowerer emits against.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{
    CanonicalDeclarationPathSelector, DeclarationMember, DeclarationMemberDef,
    DeclarationMemberShape, DurableEnumMemberShape, DurableIndexComponent, DurableIndexShape,
    DurableValueShape, FieldDef, ImageDraft, ImageType, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, RootOccurrenceSelector, Scalar, SemanticTarget, bounds,
};
use marrow_project::{FileIdentity, IdentityAnchor, IdentityKind, IdentityLedger};
use marrow_syntax::{
    FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, StoreDecl,
    TypeExpr,
};

use crate::analysis::FileRef;
use crate::decl::{
    Binding, DeclarationBudget, DeclarationIndexDrift, DeclarationLedger, DeclarationNamespace,
    DeclarationOccurrence, DeclarationRefusalId, DeclarationRefusalSummary, DeclarationSite,
    RefusalReport, refuse_covered, refuse_row,
};
use crate::demand::{DurableNaming, PathSigil};
use crate::diag::{DiagnosticCollector, IdentityGap, SourceDiagnostic};
use crate::scalar::ScalarType;
use crate::types::{
    BuildError, GArg, GenericInvariant, RecordInfo, TypeMetadataSession, TypeRegistry,
};

/// The application's fixed ledger anchor path: one local application per
/// project, so the anchor is the project itself.
const APPLICATION_ANCHOR_PATH: &str = ".";

/// The most managed indexes one `store` root may declare. The checker owns this product
/// limit; it sits well below the image's structural `MAX_INDEXES` decode bound (32), which
/// stays as headroom. `8` keeps a root's per-write index maintenance bounded and small while
/// comfortably covering the identity-plus-a-few-secondary-orderings shape narrow indexes are
/// for.
const MAX_STORE_INDEXES: usize = 8;

/// One top-level stored field as an index-projection candidate: its source name, its
/// ledger id, and the base scalar of its stored value when that value is an orderable
/// durable-key scalar.
struct IndexFieldLeaf {
    name: String,
    id: LedgerIdBytes,
    scalar: Option<ScalarType>,
}

/// One admitted component of a managed-index projection. Its durable identity
/// reference and lowerer-facing scalar travel together so admission cannot produce a
/// component whose projection type is missing.
#[derive(Clone, Copy)]
struct ResolvedIndexComponent {
    component: DurableIndexComponent,
    scalar: ScalarType,
}

/// A resolved managed index: its image shape (for the durable identity), its source
/// name, and its projected components' scalar types in order. The projection lets the
/// lowerer type-check a source index-read operand list; the site is attached later.
struct BuiltIndex {
    shape: DurableIndexShape,
    name: String,
    projection: Vec<ScalarType>,
}

/// A managed index as the lowerer reads it: its source name, unique flag, the canonical
/// declaration path of its index node, and its projected components' scalar types in
/// projection order. A nonunique projection ends with the root's identity keys; the scan
/// holds the leading field components as a prefix and yields the identity suffix as the
/// source-root `Id(^root)`.
///
/// The path is a selector, not a minted operand: the read site (a scan site for a
/// nonunique index, a lookup site for a unique one) is bound against the owning root
/// occurrence and requested at the instruction that names it.
pub(crate) struct DurableIndex {
    pub(crate) name: String,
    pub(crate) unique: bool,
    pub(crate) path: CanonicalDeclarationPathSelector,
    pub(crate) projection: Vec<ScalarType>,
}

/// The compiler scalar carried by an orderable durable-key stored shape. This stored
/// shape is the sole index-eligibility classifier: a nominal has already erased to
/// `int`, while a dense struct, closed enum, duration, or other non-key value returns
/// `None`.
fn orderable_key_scalar(value: &DurableValueShape) -> Option<ScalarType> {
    match value {
        DurableValueShape::Scalar(Scalar::Int) => Some(ScalarType::Int),
        DurableValueShape::Scalar(Scalar::Text) => Some(ScalarType::Text),
        DurableValueShape::Scalar(Scalar::Bool) => Some(ScalarType::Bool),
        DurableValueShape::Scalar(Scalar::Bytes) => Some(ScalarType::Bytes),
        DurableValueShape::Scalar(Scalar::Date) => Some(ScalarType::Date),
        DurableValueShape::Scalar(Scalar::Instant) => Some(ScalarType::Instant),
        DurableValueShape::Scalar(Scalar::Duration)
        | DurableValueShape::Struct(_)
        | DurableValueShape::Enum { .. } => None,
    }
}

/// One resolved durable field. The field-leaf operation site is not pre-minted: the
/// field carries its canonical declaration path, and the lowerer binds it against the
/// owning root occurrence and allocates (and deduplicates) a field-leaf site through the
/// draft the first time an instruction addresses it, so the image carries a leaf site per
/// *referenced* field rather than one per declared field.
pub(crate) struct DurableField {
    pub(crate) name: String,
    pub(crate) path: CanonicalDeclarationPathSelector,
    /// The field's resolved value type: a scalar, or a widened composite (a dense
    /// `struct`, or a closed `enum`/`Option`/`Result`). The lowerer builds the read
    /// result and written-value type from it.
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// One resolved scalar field of an executable branch entry: its source name, value
/// scalar, required flag, and canonical declaration path. The whole-payload
/// create/replace flows through the branch's materialized record; `path` names the
/// field-exact leaf a `^root(k).branch(bk).field` read or write addresses directly, one
/// level below the root, whose site the lowerer binds and allocates on first reference.
pub(crate) struct DurableBranchField {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
    pub(crate) path: CanonicalDeclarationPathSelector,
}

/// One scalar/widened leaf of an executable root-level `group`: its source name, value
/// type (a scalar or a widened composite), and required flag. A leaf is not addressed by
/// a durable site of its own — a group-leaf access reads or rewrites the whole group — so
/// it carries no site, only the shape a group-leaf read projects and a group-leaf write
/// stores into the group record's slot.
pub(crate) struct DurableGroupLeaf {
    pub(crate) name: String,
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// One executable root-level unkeyed `group` of a flat-executable root: a value unit of
/// the root entry addressed by the root's own key-path (a group is markerless — its
/// presence is the entry's presence). Its whole read/replace/erase address the
/// `GroupEntry` site bound at the group's canonical declaration `path`; a group-leaf
/// access `^root(k).group.leaf` is a whole-group read-modify-write over the materialized
/// group `record`, so a leaf never has a durable site of its own.
pub(crate) struct DurableGroup {
    pub(crate) name: String,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) path: CanonicalDeclarationPathSelector,
    pub(crate) fields: Vec<DurableGroupLeaf>,
}

impl DurableGroup {
    /// The declaration-order slot index and descriptor of the group leaf `name` — the
    /// slot into the group's materialized record a leaf read projects and a leaf write
    /// rewrites, so a group-leaf operation addresses the same slot the record types.
    pub(crate) fn field_index(&self, name: &str) -> Option<(u16, &DurableGroupLeaf)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| (index as u16, field))
    }
}

/// One executable keyed `branch` of a flat-executable root: a scalar-field keyed
/// scalar-field subtree one or more levels below the root, carrying its own nested
/// branches recursively. Its whole-entry operations address the key-path
/// `[root_key, branch_key, …]` through the whole-payload site bound at its canonical
/// declaration `path`, and its constructor `Resource.branch.…(field: value, …)` builds
/// `record` from `fields` in declaration order.
pub(crate) struct DurableBranch {
    pub(crate) name: String,
    /// The branch's ordered key columns (one or more), the whole composite branch key.
    pub(crate) key: Vec<ScalarType>,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) path: CanonicalDeclarationPathSelector,
    pub(crate) fields: Vec<DurableBranchField>,
    pub(crate) branches: Vec<DurableBranch>,
}

impl DurableBranch {
    pub(crate) fn field(&self, name: &str) -> Option<&DurableBranchField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The nested branch declared with the simple name `name`, if any.
    pub(crate) fn branch(&self, name: &str) -> Option<&DurableBranch> {
        self.branches.iter().find(|branch| branch.name == name)
    }
}

/// One executable durable root, its operation sites, its executable root-level groups,
/// and its executable branches. A keyed root (any key arity) whose top-level fields are
/// scalars or widened values, whose root-level groups hold only such fields, and whose
/// only nested placements are field-only keyed branches reaches this form; its key columns
/// back the kernel-serviceable read/write path, each group is a value unit of the root
/// entry, and each branch adds its own key tuple below it.
pub(crate) struct DurableRoot {
    pub(crate) name: String,
    /// This root's DURABLE-table index (its declaration-ordered RootId) — the discriminant
    /// an entry identity `Id(^root)` carries, so two identities over different roots are
    /// distinct values and an identity addressed to the wrong root is a type error.
    pub(crate) root_id: u16,
    /// The resource (product) name backing this store — the head of a branch's
    /// qualified constructor path `Resource.branch(…)`.
    pub(crate) resource: String,
    /// The root's ordered key columns (one or more), the whole composite root key.
    pub(crate) key: Vec<ScalarType>,
    pub(crate) record: marrow_image::TypeId,
    /// The root occurrence this store declaration admitted. Every site over this root —
    /// its own whole payload, its members', and its managed indexes' — is bound against
    /// it, so a site is occurrence-qualified even when two roots share one Product
    /// declaration.
    pub(crate) occurrence: RootOccurrenceSelector,
    /// The canonical path of this root's own keyed placement.
    pub(crate) placement: CanonicalDeclarationPathSelector,
    pub(crate) fields: Vec<DurableField>,
    pub(crate) groups: Vec<DurableGroup>,
    pub(crate) branches: Vec<DurableBranch>,
    pub(crate) indexes: Vec<DurableIndex>,
}

impl DurableRoot {
    pub(crate) fn field(&self, name: &str) -> Option<&DurableField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The executable root-level group declared with the simple name `name`, if any —
    /// the owner a group whole access `^root(k).group` or a group-leaf access
    /// `^root(k).group.leaf` resolves against.
    pub(crate) fn group(&self, name: &str) -> Option<&DurableGroup> {
        self.groups.iter().find(|group| group.name == name)
    }

    /// The executable branch declared with the simple name `name`, if any.
    pub(crate) fn branch(&self, name: &str) -> Option<&DurableBranch> {
        self.branches.iter().find(|branch| branch.name == name)
    }

    /// The managed index declared with the simple name `name`, if any — the owner a
    /// source index read (`^root.name[…]`) resolves against.
    pub(crate) fn index(&self, name: &str) -> Option<&DurableIndex> {
        self.indexes.iter().find(|index| index.name == name)
    }
}

/// One `store` declaration the registry admitted: the position of its executable
/// descriptor when the kernel serves its shape, and nothing more when the root is
/// parked (a singleton, a nominal-typed field, or a group nested in a branch or
/// another group). A parked root carries a complete identity and a full site set, so
/// an operation over it is a precise "not yet executable" rejection rather than an
/// unknown name.
pub(crate) struct DeclaredRoot {
    executable: Option<usize>,
}

/// What a `^name` reference resolves to in the durable root namespace.
///
/// The three failing answers are distinct facts about the source and they are held
/// apart here rather than recovered by a chain of probes: the kernel cannot yet serve
/// a declared shape, the declaration itself was refused and already reported its
/// cause, or no store of that name is declared anywhere.
pub(crate) enum RootBinding<'a> {
    /// Admitted, with a complete identity, and kernel-serviceable.
    Executable(&'a DurableRoot),
    /// Admitted with a complete identity, but outside the executable subset.
    NotYetExecutable,
    /// DeclarationSite and refused. The declaration reported the cause; a use reuses it,
    /// carrying the handle that lets a type-resolution result name it.
    Refused(DeclarationRefusalId, &'a DeclarationRefusalSummary),
    /// No store of this name is declared — the one case a not-in-scope report may
    /// describe.
    Absent,
}

/// One field of a durable Product's materialized branch entry record: its source name,
/// value scalar, and required flag.
///
/// These are Product *declaration* facts. They carry no operation site and no semantic
/// path: those are occurrence facts, and one Product declaration may be projected by
/// several store roots.
pub(crate) struct BranchRecordField {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
}

/// The materialized entry record of one keyed `branch` of a durable Product: the field
/// layout a value of that record projects, in declaration order, and the simple names of
/// the branch's own keyed sub-branches.
///
/// This is the whole capability a *record-shape* query may have. Reading a field of a
/// materialized branch entry value, and building one with `Resource.branch(…)`, are
/// questions about what the Product declares — they name no store root, and a branch
/// entry record type names none either, since one Product declaration mints one such
/// record however many roots project it. An operation on a durable node is addressed
/// through the occurrence it was resolved against instead.
pub(crate) struct BranchRecordShape {
    record: marrow_image::TypeId,
    fields: Vec<BranchRecordField>,
    sub_branches: Vec<String>,
}

impl BranchRecordShape {
    /// The branch entry's materialized record type.
    pub(crate) fn record(&self) -> marrow_image::TypeId {
        self.record
    }

    /// The declared fields, in declaration order — the record's slot order.
    pub(crate) fn fields(&self) -> &[BranchRecordField] {
        &self.fields
    }

    /// The declared field named `name`, if any.
    pub(crate) fn field(&self, name: &str) -> Option<&BranchRecordField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The declaration-order slot index and descriptor of the field `name` — the slot a
    /// field read of a materialized branch entry value projects.
    pub(crate) fn field_index(&self, name: &str) -> Option<(u16, &BranchRecordField)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| (index as u16, field))
    }

    /// Whether the branch declares a keyed sub-branch named `name`. A sub-branch is a
    /// distinct durable node, not a field of this record, so a selector naming one is
    /// steered to the durable-path form.
    pub(crate) fn declares_branch(&self, name: &str) -> bool {
        self.sub_branches.iter().any(|branch| branch == name)
    }
}

/// What a resource spelling binds as a durable Product declaration.
///
/// The question is about the Product, not about any one root: several `store` declarations
/// may project one resource, and the declaration facts they share are the same whichever of
/// them is admitted. A resource is `Refused` only when *no* store over it was admitted — one
/// refused store beside an admitted sibling steers nothing, because the Product's shape is
/// known.
///
/// Unlike [`RootBinding`] there is no `NotYetExecutable` answer and `Declared` carries no
/// root. Whether the kernel can serve a root is a fact about that occurrence; a Product's
/// declared members, branches, and entry records are the same either way, so splitting the
/// admitted answer on executability made a declaration query report an occurrence scan's
/// result — a Product whose stores are all keyless answered as though it declared nothing.
pub(crate) enum ProductBinding<'a> {
    /// At least one store over this resource was admitted, so the Product's declaration
    /// facts are available.
    Declared,
    /// Every store over this resource was refused; the first refusal carries the cause
    /// the use is steered to.
    Refused(&'a DeclarationRefusalSummary),
    /// No store binds this resource.
    Absent,
}

/// The `store` declarations that bind one resource, in declaration order.
struct ProductStores {
    /// The placement names of the stores over this resource the compiler admitted.
    admitted: Vec<String>,
    /// The placement name of the first store over this resource the compiler refused.
    /// Consulted only when nothing was admitted.
    first_refused: Option<String>,
    /// Whether this Product's branch entry records have been recorded, which its first
    /// executable root does once for every root that projects it.
    declared_branches: bool,
}

/// The durable registry: every declared `store` root, in declaration order.
///
/// `roots` holds the flat keyed roots the kernel can serve; `declared` is the ledger
/// of every declared placement name, admitted or refused. A refused store keeps its
/// name there with the cause its declaration reported, so a later `^name` reference
/// is answered with that cause instead of reading as a name that was never written.
/// A root's index in the draft's DURABLE table is its declaration order (RootId), so
/// the executable list stays declaration-ordered.
pub(crate) struct DurableRegistry {
    roots: Vec<DurableRoot>,
    declared: DeclarationLedger<String, DeclaredRoot>,
    /// The `store` declarations binding each resource, appended in the same statement
    /// as the ledger entry so the two cannot drift.
    ///
    /// The ledger stays the sole authority for what a placement name binds; this only
    /// lets a resource-keyed lookup reach it. A placement here the ledger does not know
    /// is [`DeclarationIndexDrift`], not a neighbouring root.
    products: BTreeMap<String, ProductStores>,
    /// Every durable Product's materialized branch entry records, keyed by record type
    /// and by the branch's qualified constructor path (`Book.notes.tags`).
    ///
    /// A Product declaration mints one entry record per declared branch however many
    /// roots project it, so this table is declaration-scoped: it is written once, at
    /// each Product's first executable root, and holds no site, path, or root.
    branch_records: BTreeMap<marrow_image::TypeId, BranchRecordShape>,
    branch_paths: BTreeMap<String, marrow_image::TypeId>,
    /// Every durable Product's declared keyed-branch paths in qualified source spelling
    /// (`Book.notes`, `Book.notes.tags`), written once per Product at its first admitted
    /// root, straight from the resource declaration.
    ///
    /// Whether a Product declares a branch named `n` is a *declaration* question, and it
    /// has one answer however many roots occur over the Product and whichever of them the
    /// kernel can serve. Reading it out of [`Self::branch_paths`] answered it from an
    /// executable-occurrence scan instead: that table is written from a root's built branch
    /// descriptors, which exist only for a root inside the executable subset, so a Product
    /// whose stores are all keyless carries a complete declared branch tree and still
    /// answered "no branch". Materialized-record shape stays keyed by record type — a
    /// materialized branch entry value only arises from an executable branch — but the
    /// declared-name question is answered here.
    declared_branch_paths: BTreeSet<String>,
    /// The durable-path naming join for every admitted graph node, `(ledger id, sigil,
    /// simple name)`, accumulated across the project's admitted stores. The
    /// [`DurableNaming`] the demand sentence spells paths through is built from this.
    naming: Vec<(LedgerIdBytes, PathSigil, String)>,
}

impl DurableRegistry {
    /// A registry with no declared root, charging its retentions against the pass's
    /// `budget`. There is no `Default`: a ledger that retains off the pass's books
    /// would let the declared ceiling be crossed without reporting it.
    pub(crate) fn empty(budget: DeclarationBudget) -> Self {
        Self {
            roots: Vec::new(),
            declared: DeclarationLedger::new(DeclarationNamespace::DurableRoot, budget),
            products: BTreeMap::new(),
            branch_records: BTreeMap::new(),
            branch_paths: BTreeMap::new(),
            declared_branch_paths: BTreeSet::new(),
            naming: Vec::new(),
        }
    }
}

impl DurableRegistry {
    /// The compiler-owned join from each admitted durable node's ledger id to its source
    /// spelling, so a verifier-reconstructed demand set can be described in the program's
    /// own `^root.member` spelling.
    pub(crate) fn naming(&self) -> DurableNaming {
        DurableNaming::from_entries(self.naming.clone())
    }

    /// What the placement name `name` resolves to: its executable root, a parked
    /// declaration, the refusal that stands in its place, or a genuine absence. The
    /// one owner of that four-way answer; every other root lookup projects from it.
    pub(crate) fn root(&self, name: &str) -> Result<RootBinding<'_>, DeclarationIndexDrift> {
        Ok(match self.declared.lookup(name)? {
            Binding::Accepted(declared) => match declared.executable {
                Some(at) => match self.roots.get(at) {
                    Some(root) => RootBinding::Executable(root),
                    // Layer 1 and the executable list disagree. Answering "declared
                    // but parked" is the truthful reading of an admitted root whose
                    // descriptor cannot be produced, and it reports rather than
                    // fabricating an absence.
                    None => RootBinding::NotYetExecutable,
                },
                None => RootBinding::NotYetExecutable,
            },
            Binding::Refused(id, refusal) => RootBinding::Refused(id, refusal),
            Binding::Absent => RootBinding::Absent,
        })
    }

    /// The refusal a durable-root handle addresses. A handle another namespace
    /// minted is drift here, checked by the ledger's own tag.
    pub(crate) fn refusal(
        &self,
        id: DeclarationRefusalId,
    ) -> Result<&DeclarationRefusalSummary, DeclarationIndexDrift> {
        self.declared.refusal(id)
    }

    /// The executable flat keyed root declared with the placement name `name`, if any —
    /// the owner an entry address `^name[…]` resolves against. The probe-free form, for
    /// the classifiers that resolve a shape without reporting; a lookup that reports
    /// takes [`DurableRegistry::root`] so a refused root is not read as an absent one.
    pub(crate) fn root_by_name(
        &self,
        name: &str,
    ) -> Result<Option<&DurableRoot>, DeclarationIndexDrift> {
        Ok(match self.root(name)? {
            RootBinding::Executable(root) => Some(root),
            RootBinding::NotYetExecutable | RootBinding::Refused(..) | RootBinding::Absent => None,
        })
    }

    /// What the resource `resource` declares as a durable Product, reached through the
    /// store declarations that bind it.
    ///
    /// The owner of a branch constructor `Resource.branch(…)` and of the branch steer on
    /// a materialized entry value. Both are questions about what the Product declares, so
    /// any admitted store over the resource answers them: the branch tree they carry is
    /// the resource's own. A store this project declared and the compiler refused answers
    /// `Refused` only when no sibling store over the same resource was admitted — one
    /// store's cause must not be sent to a reader whose Product is fine.
    ///
    /// One admitted store is the whole `Declared` answer. Nothing here consults the
    /// executable list: the loop over the admitted names is a drift guard on the projection
    /// table, not the source of the verdict.
    ///
    /// `Absent` means *no store binds this resource*, which only the missing projection
    /// entry establishes. A projection entry naming a placement the ledger does not know
    /// is the two having drifted — they are written in the same statement — and is
    /// reported rather than answered.
    pub(crate) fn product(
        &self,
        resource: &str,
    ) -> Result<ProductBinding<'_>, DeclarationIndexDrift> {
        let Some(stores) = self.products.get(resource) else {
            return Ok(ProductBinding::Absent);
        };
        for name in &stores.admitted {
            match self.root(name)? {
                RootBinding::Executable(_) | RootBinding::NotYetExecutable => {}
                RootBinding::Refused(..) | RootBinding::Absent => {
                    return Err(DeclarationIndexDrift);
                }
            }
        }
        if !stores.admitted.is_empty() {
            return Ok(ProductBinding::Declared);
        }
        let Some(refused) = &stores.first_refused else {
            return Err(DeclarationIndexDrift);
        };
        match self.root(refused)? {
            RootBinding::Refused(_, summary) => Ok(ProductBinding::Refused(summary)),
            RootBinding::Executable(_) | RootBinding::NotYetExecutable | RootBinding::Absent => {
                Err(DeclarationIndexDrift)
            }
        }
    }

    /// The materialized entry record of the branch reached by the branch-name `path`
    /// from `resource` — the shape `Resource.notes.tags(…)` constructs. Declaration
    /// facts only: the constructor builds a record, it addresses no durable node.
    pub(crate) fn branch_record_at(
        &self,
        resource: &str,
        path: &[&str],
    ) -> Option<&BranchRecordShape> {
        if path.is_empty() {
            return None;
        }
        let mut qualified = String::from(resource);
        for step in path {
            qualified.push('.');
            qualified.push_str(step);
        }
        self.branch_record(*self.branch_paths.get(&qualified)?)
    }

    /// Whether `resource` declares a keyed branch named `name` directly below itself — a
    /// Product declaration fact, answered from the declared branch paths rather than from
    /// any one root's built descriptors.
    pub(crate) fn declares_branch(&self, resource: &str, name: &str) -> bool {
        self.declared_branch_paths
            .contains(&format!("{resource}.{name}"))
    }

    /// The materialized branch entry record `ty` types, if it is one — the shape a field
    /// read of a bound `if const n = ^root(k).branch(bk)` value projects.
    ///
    /// One Product declaration mints one such record however many roots project it, so
    /// this answers a record-shape question and never names an occurrence. The durable
    /// node an operation addresses comes from the address that was resolved, not from the
    /// type of a value it materialized.
    pub(crate) fn branch_record(&self, ty: marrow_image::TypeId) -> Option<&BranchRecordShape> {
        self.branch_records.get(&ty)
    }

    /// The executable root whose declaration-ordered RootId is `root_id` — the root an
    /// entry identity `Id(^root)` carries, so a `place` bound to an identity operand can
    /// recover the root's ordered key scalars for the columns the identity spreads into.
    ///
    /// A root's index in the executable list *is* its RootId, so this is a keyed lookup
    /// rather than a scan. `None` for an id no executable root carries.
    pub(crate) fn root_by_id(&self, root_id: u16) -> Option<&DurableRoot> {
        self.roots.get(usize::from(root_id))
    }

    /// Every declared store-root name — admitted, parked, or refused — so a reference
    /// to an unknown `^root` can offer the nearest declared root as a did-you-mean. A
    /// refused root is in the corpus because it is in the source: dropping it would
    /// leave a near miss on a name the reader can see with no correction at all.
    pub(crate) fn root_names(&self) -> impl Iterator<Item = &str> {
        self.declared.keys().map(String::as_str)
    }

    /// Record one Product declaration's branch entry records from the branch tree of its
    /// first executable root, keyed by record type and by qualified constructor path.
    ///
    /// Only declaration facts cross: the branch's simple name, key-free field layout, and
    /// materialized record type, plus the names of its own sub-branches. Operation sites
    /// and semantic paths stay on the occurrence that owns them.
    fn record_branch_declarations(&mut self, container: &str, branches: &[DurableBranch]) {
        for branch in branches {
            let qualified = format!("{container}.{}", branch.name);
            self.branch_paths.insert(qualified.clone(), branch.record);
            self.branch_records.insert(
                branch.record,
                BranchRecordShape {
                    record: branch.record,
                    fields: branch
                        .fields
                        .iter()
                        .map(|field| BranchRecordField {
                            name: field.name.clone(),
                            scalar: field.scalar,
                            required: field.required,
                        })
                        .collect(),
                    sub_branches: branch
                        .branches
                        .iter()
                        .map(|nested| nested.name.clone())
                        .collect(),
                },
            );
            self.record_branch_declarations(&qualified, &branch.branches);
        }
    }

    /// Record the qualified paths of every keyed branch `members` declares below
    /// `container`, recursively. Source declaration facts only: no key column, field type,
    /// record, site, or root is read, so the answer is available for every admitted Product
    /// whether or not a root over it reached the executable subset.
    fn record_declared_branch_paths(&mut self, container: &str, members: &[ResourceMember]) {
        for member in members {
            let ResourceMember::Group(group) = member else {
                continue;
            };
            if group.keys.is_empty() {
                continue;
            }
            let qualified = format!("{container}.{}", group.name);
            self.record_declared_branch_paths(&qualified, &group.members);
            self.declared_branch_paths.insert(qualified);
        }
    }

    /// Build the registry from the project's store declarations, adding each admitted
    /// root and its complete ledger identity block to the draft in declaration order (so
    /// a root's DURABLE-table index is its RootId). A store whose placement name repeats
    /// an earlier one is a precise `check.type` rejection and does not enter the draft;
    /// an index, a missing or mismatched resource, a key column outside the closed
    /// orderable durable-key set, or a key tuple past the column bound reject that one
    /// store — and so does a durable graph whose identity is incomplete: every durable
    /// declaration (the application, the root placement, its product, each key column,
    /// each stored field, each group namespace, and each nested branch placement and key
    /// column) must have a live row in the committed `.marrow/ids` ledger, or the
    /// declaration fails precisely with `check.durable_identity`. A store that fails
    /// validation contributes only its diagnostic; the other stores' roots stand, so one
    /// store's gap never erases the whole registry. The compiler only *reads* the ledger;
    /// minting lives in the `marrow run` convenience action (and in the accepted apply
    /// action when it lands).
    pub(crate) fn build(
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        resources: &[(FileRef, FileIdentity, &ResourceDecl)],
        stores: &[(FileRef, FileIdentity, &StoreDecl)],
        ledger: Option<&IdentityLedger>,
        diagnostics: &mut DiagnosticCollector,
        budget: DeclarationBudget,
    ) -> Result<Self, BuildError> {
        if stores.is_empty() {
            return Ok(Self::empty(budget));
        }
        records.with_metadata_session(|metadata| {
            let mut registry = Self::empty(budget.clone());
            let mut type_metadata = DurableTypeMetadata { records, metadata };
            let mut reported_identity_gaps = BTreeSet::new();
            let mut identity_build = IdentityBuildState {
                ledger,
                reported_gaps: &mut reported_identity_gaps,
            };
            for (at, file, store) in stores {
                let declared = DeclarationSite {
                    name: &store.root.root,
                    file,
                    at: *at,
                    span: store.root.span,
                };
                // A repeated placement name has no unambiguous address and cannot key a
                // second DURABLE-table row; reject it and keep the first declaration. A
                // refused declaration still occupies its name, so the repeat conflicts
                // whichever of the two the compiler could admit.
                if registry.declared.declared(store.root.root.as_str()) {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        store.root.span,
                        format!(
                            "store root `^{}` is declared more than once; each store root has a \
                             distinct name",
                            store.root.root
                        ),
                    ));
                    continue;
                }
                let occurrence = match build_one(
                    draft,
                    &mut type_metadata,
                    resources,
                    declared,
                    store,
                    &mut identity_build,
                    diagnostics,
                )? {
                    StoreBuild::Admitted(built) => {
                        registry.naming.extend(built.naming);
                        let executable = built.executable.map(|root| {
                            registry.roots.push(root);
                            registry.roots.len() - 1
                        });
                        DeclarationOccurrence::Accepted(DeclaredRoot { executable })
                    }
                    StoreBuild::Refused(refusal) => DeclarationOccurrence::Refused(refusal),
                };
                // The resource projection is appended in the same statement as the
                // ledger entry, so a store cannot be declared without being reachable
                // by the resource it binds.
                let stores = registry
                    .products
                    .entry(store.resource.clone())
                    .or_insert_with(|| ProductStores {
                        admitted: Vec::new(),
                        first_refused: None,
                        declared_branches: false,
                    });
                let mut declare_branches = None;
                let mut declare_branch_paths = false;
                match &occurrence {
                    DeclarationOccurrence::Accepted(DeclaredRoot { executable }) => {
                        declare_branch_paths = stores.admitted.is_empty();
                        stores.admitted.push(store.root.root.clone());
                        // A Product declaration mints one materialized entry record per
                        // declared branch however many roots project it, so its branch
                        // record table is written at that Product's first executable root
                        // and never again. Every later occurrence carries the identical
                        // declaration and its own operation sites, and no site is here.
                        if let Some(at) = executable
                            && !std::mem::replace(&mut stores.declared_branches, true)
                        {
                            declare_branches = Some(*at);
                        }
                    }
                    DeclarationOccurrence::Refused(_) => {
                        stores.first_refused.get_or_insert(store.root.root.clone());
                    }
                }
                if declare_branch_paths
                    && let Some((_, _, decl)) =
                        resources.iter().find(|(_, _, d)| d.name == store.resource)
                {
                    registry.record_declared_branch_paths(&store.resource, &decl.members);
                }
                if let Some(at) = declare_branches {
                    let branches = std::mem::take(&mut registry.roots[at].branches);
                    registry.record_branch_declarations(&store.resource, &branches);
                    registry.roots[at].branches = branches;
                }
                registry
                    .declared
                    .declare(store.root.root.clone(), occurrence)?;
            }
            Ok(registry)
        })
    }
}

/// One store declaration's build outcome.
enum StoreBuild {
    /// The graph is admissible: its identity is complete and its root (with any executable
    /// descriptor) entered the draft.
    Admitted(BuiltRoot),
    /// The store was refused, with the cause its declaration reported. There is one
    /// refusal outcome and no silent one: a store that leaves no trace in the registry
    /// is what makes every later `^name` reference read as a name never written.
    Refused(DeclarationRefusalSummary),
}

/// Why one `store` declaration was refused.
///
/// Grouped by what a later `^root` reference must be told, not one variant per site:
/// only the identity class has a report *family* to send the reader to, and every
/// other class reuses the single declaring row it pushed. Nine of the ten sites that
/// mark a durable graph incomplete are not identity gaps, and this is what keeps them
/// from claiming to be.
enum DurableRefusal<'a> {
    /// A durable anchor has no live row in the committed ledger. The one class
    /// entitled to send a use to the `check.durable_identity` reports.
    ///
    /// `report` names where the row the steer cites was made: at this declaration,
    /// or by the earlier store that first reached this project-wide anchor. The gap
    /// refuses this root either way; only the reporting is deduplicated.
    IdentityIncomplete {
        anchor: IdentityAnchor,
        retired: bool,
        report: RefusalReport,
    },
    /// A field, group leaf, key tuple, or durable value in the root's stored shape is
    /// outside the closed durable value set.
    Member { subject: &'a str, span: SourceSpan },
    /// A managed index violates the closed narrow-index admission rules.
    Index { message: String, span: SourceSpan },
    /// A fixed compiler-owned bound on the stored shape was crossed.
    Bound { message: String, span: SourceSpan },
    /// A durable value cycle. This is the one refusal that pushes no row of its own:
    /// its cause is the `check.recursion` report from `types::reject_value_cycles`,
    /// which runs after lowering so that it also sees the instantiations lowering
    /// mints. The steer names that code without a location.
    ValueCycle,
    /// The store failed admission before its graph was walked: a missing or mismatched
    /// resource, an out-of-range key tuple, or a key column outside the closed
    /// orderable durable-key set. Each of these built its own row.
    Admission { row: SourceDiagnostic },
}

/// Report one store refusal and summarize it from that same report, so a retained
/// refusal can never describe a diagnostic that was not made.
///
/// Exhaustive by match: a new refusal class is a build error here rather than a class
/// that silently renders as some other class's cause.
fn refuse_store(
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    refusal: DurableRefusal<'_>,
) -> DeclarationRefusalSummary {
    match refusal {
        DurableRefusal::IdentityIncomplete {
            anchor,
            retired,
            report,
        } => {
            let gap = IdentityGap {
                kind: anchor.kind,
                path: anchor.path.clone(),
                retired,
            };
            let summary = match report {
                RefusalReport::AtDeclaration => refuse_row(
                    diagnostics,
                    at,
                    identity_gap(at.file, at.span, anchor.kind, &anchor.path, retired),
                ),
                // Covered by the first store to reach this project-wide anchor, which
                // pushed the `check.durable_identity` row this refusal names.
                _ => refuse_covered(at, Code::CheckDurableIdentity.as_str()),
            };
            summary.with_gap(gap)
        }
        DurableRefusal::Member { subject, span } => {
            refuse_row(diagnostics, at, unsupported(at.file, span, subject))
        }
        DurableRefusal::Index { message, span } => refuse_row(
            diagnostics,
            at,
            SourceDiagnostic::at(Code::CheckType.as_str(), at.file, span, message),
        ),
        DurableRefusal::Bound { message, span } => {
            refuse_row(diagnostics, at, resource_limit(at.file, span, message))
        }
        // Covered by `types::reject_value_cycles` (compile.rs), which reports every
        // durable value cycle after lowering.
        DurableRefusal::ValueCycle => refuse_covered(at, Code::CheckRecursion.as_str()),
        DurableRefusal::Admission { row } => refuse_row(diagnostics, at, row),
    }
}

/// One store declaration's admitted build: the placement name that entered the draft and,
/// when the root is a flat kernel-serviceable shape, its executable descriptor.
struct BuiltRoot {
    executable: Option<DurableRoot>,
    /// This store's durable-path naming entries — its root placement, stored fields,
    /// managed indexes, groups, and keyed branches — committed only for an admitted graph.
    naming: Vec<(LedgerIdBytes, PathSigil, String)>,
}

/// The immutable type owner and its one operation-local validation session.
/// Durable construction passes them together so no store can silently open a
/// second metadata directory while the registry remains unchanged.
struct DurableTypeMetadata<'registry, 'session> {
    records: &'registry TypeRegistry,
    metadata: &'session mut TypeMetadataSession<'registry>,
}

/// Project-wide identity inputs shared by each store build. The ledger remains
/// read-only while the gap set assigns the first diagnostic for a shared anchor
/// across all per-store resolvers.
struct IdentityBuildState<'ledger, 'gaps> {
    ledger: Option<&'ledger IdentityLedger>,
    reported_gaps: &'gaps mut BTreeSet<IdentityAnchor>,
}

/// Where this occurrence's Product declaration comes from: the rows the draft already
/// holds, or the command vector this root is the first to state.
///
/// The two are exclusive by construction — a draft either holds the declaration or does
/// not — and both orders place the resource's top-level fields first, in record order, so
/// the leading run reads the same either way.
enum ProductDeclarationSource {
    /// The declaration the draft already holds, read back from an earlier root over this
    /// same Product. Nothing further is admitted for it.
    Held(Vec<DeclarationMember>),
    /// The command vector this root just resolved. It is admitted once this store's
    /// identity is known complete, and the admission answers with the same member rows a
    /// later root reads back as [`ProductDeclarationSource::Held`].
    Built(Vec<DeclarationMemberDef>),
}

impl ProductDeclarationSource {
    /// The declared shape of each direct member, in declaration order, whichever form the
    /// declaration is currently in.
    fn member_shapes(&self) -> Vec<&DeclarationMemberShape> {
        match self {
            Self::Held(members) => members.iter().map(DeclarationMember::shape).collect(),
            Self::Built(commands) => commands.iter().map(|command| &command.shape).collect(),
        }
    }
}

/// Resolve, validate, and commit one `store` declaration into the draft, returning its
/// build outcome. A failing store pushes its diagnostic and commits no root, site, or
/// application identity, so it cannot corrupt an already-appended root (`build_extras` may
/// append record types before the gate, which is harmless — the pushed diagnostic fails
/// compilation). Every rejection yields [`StoreBuild::Refused`] carrying the summary of
/// the report it just made, so no store leaves the registry without a retrievable cause.
/// The heavy resolution runs against a local [`IdentityResolver`] and the completeness
/// gate below precedes every root/site/identity commit, so the draft is touched only once
/// the store is known admissible.
fn build_one(
    draft: &mut ImageDraft,
    type_metadata: &mut DurableTypeMetadata<'_, '_>,
    resources: &[(FileRef, FileIdentity, &ResourceDecl)],
    declared: DeclarationSite<'_>,
    store: &StoreDecl,
    identity_build: &mut IdentityBuildState<'_, '_>,
    diagnostics: &mut DiagnosticCollector,
) -> Result<StoreBuild, GenericInvariant> {
    let file = declared.file;
    let records = type_metadata.records;
    let metadata = &mut *type_metadata.metadata;
    let refuse = |diagnostics: &mut DiagnosticCollector, row| {
        Ok(StoreBuild::Refused(refuse_store(
            diagnostics,
            declared,
            DurableRefusal::Admission { row },
        )))
    };
    if store.root.keys.len() > bounds::MAX_KEY_COLUMNS {
        return refuse(
            diagnostics,
            resource_limit(
                file,
                store.root.span,
                format!(
                    "a store root key tuple has {} columns; the fixed limit is {}",
                    store.root.keys.len(),
                    bounds::MAX_KEY_COLUMNS
                ),
            ),
        );
    }
    // Resolve each root key column's scalar in declared tuple order. A singleton
    // root has no columns.
    let key_scalars = match resolve_key_scalars(file, store.root.span, &store.root.keys, records) {
        Ok(scalars) => scalars,
        Err(row) => return refuse(diagnostics, *row),
    };
    let Some(record) = records.by_name(&store.resource) else {
        return refuse(
            diagnostics,
            SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                store.span,
                format!("`{}` is not a resource in this project", store.resource),
            ),
        );
    };
    // The type registry admitted this resource by name, so the declaration it was
    // built from is in the resource set. A miss is the two owners disagreeing about
    // the same name, not a fact about the source: reporting it as a refusal would
    // charge the user for a compiler inconsistency, and staying silent would drop the
    // root with no cause at all.
    let Some((_, _, resource)) = resources
        .iter()
        .find(|(_, _, decl)| decl.name == store.resource)
    else {
        return Err(GenericInvariant::DurableResourceMissing(record.type_id));
    };

    // Compiler-owned enum readiness is validated before the first ledger lookup.
    // This keeps a malformed Ready body out of both contextual Unsupported
    // diagnostics and durable identity resolution.
    metadata.validate_durable_value_metadata(
        record.fields.iter().map(|field| field.ty).chain(
            record
                .groups
                .iter()
                .flat_map(|group| group.fields.iter().map(|field| field.ty)),
        ),
    )?;

    // Resolve the durable graph's ledger identities. The application, the root
    // placement, its product, and each root key column anchor first; then the
    // resource's member tree (top-level fields, groups, and branches) anchors as
    // it is walked. A missing or retired anchor is a precise typed diagnostic
    // carrying the `(kind, path)` gap the mint action consumes.
    let mut resolver = IdentityResolver::new(
        declared,
        store.span,
        identity_build.ledger,
        identity_build.reported_gaps,
        diagnostics,
    );
    let application = resolver.resolve(IdentityKind::Application, APPLICATION_ANCHOR_PATH);
    let placement = resolver.resolve(IdentityKind::Root, &store.root.root);
    resolver.name_step(placement, PathSigil::Root, &store.root.root);
    let product = resolver.resolve(IdentityKind::Product, &store.resource);
    let key_ids: Vec<LedgerIdBytes> = store
        .root
        .keys
        .iter()
        .map(|key_param| {
            resolver.resolve(
                IdentityKind::Key,
                &format!("{}.{}", store.root.root, key_param.name),
            )
        })
        .collect();

    // The resource's member tree, in canonical order: its top-level fields
    // (aligned with the materialized record), then its static `group`
    // namespaces, then its keyed `branch` placements — each group and branch
    // recursively holding its own members. A top-level field's value shape is
    // drawn from the closed acyclic durable value set (a nominal scalar, a dense
    // struct, a closed enum, or an `Option` of one), the field anchoring the
    // ledger id while nested product leaves are shape bytes and each durable-
    // reachable enum contributes its own sum/member identities. `has_extras`
    // records whether the resource declares any group or branch.
    //
    // A Product is a declaration and a root is an occurrence of it, so the graph is
    // built **once**, at this Product's first root in canonical store-traversal order:
    // a later root over the same Product references the declaration the draft already
    // holds, resolving no anchor a second time and — decisively — minting no second
    // entry record type for its nested branches.
    //
    // The declaration is only *admitted* into the draft once this store's identity is
    // known complete (below). A graph with an unresolved anchor carries placeholder ids,
    // including a placeholder Product identity, so admitting it would let one refused
    // store's declaration answer for every other refused store's resource.
    let source = match draft.product_members(product) {
        Some(members) => ProductDeclarationSource::Held(members),
        None => ProductDeclarationSource::Built(
            resolver.build_product_graph(draft, records, metadata, store, resource, record),
        ),
    };
    if let Some(invariant) = resolver.invariant {
        return Err(invariant);
    }

    // Resolve the root's managed indexes before appending the group/branch members
    // (an index projects only the root's identity keys and top-level fields, so it
    // resolves against exactly those leaves). `members[0..record.fields.len()]` is
    // the top-level field member set, in record order, so each field's ledger id
    // and value shape is read from it. An index admission violation is a precise
    // `check.type` diagnostic that also marks the graph incomplete, so a rejected
    // index discards the whole durable graph rather than emitting a partial one.
    let key_entries: Vec<(String, LedgerIdBytes, ScalarType)> = store
        .root
        .keys
        .iter()
        .zip(&key_ids)
        .zip(&key_scalars)
        .map(|((key_param, id), scalar)| (key_param.name.clone(), *id, *scalar))
        .collect();
    // The Product's members, however they were reached: the command vector this store
    // just built, or the declaration the draft already holds. Both place the resource's
    // top-level fields first, in record order, so the leading run reads the same either
    // way.
    let member_shapes: Vec<&DeclarationMemberShape> = source.member_shapes();
    let field_entries: Vec<IndexFieldLeaf> = record
        .fields
        .iter()
        .zip(&member_shapes)
        .map(|(field, shape)| {
            let (id, value) = match shape {
                DeclarationMemberShape::Field { id, value, .. } => (*id, value),
                #[expect(
                    clippy::unreachable,
                    reason = "match-arm narrowing: this map zips the leading top-level-field members of the Product declaration, which the command vector places before its groups and branches, so every zipped member is a `Field`"
                )]
                _ => unreachable!("the first members are the record's top-level fields"),
            };
            IndexFieldLeaf {
                name: field.name.clone(),
                id,
                scalar: orderable_key_scalar(value),
            }
        })
        .collect();
    // Each top-level stored field is a path-step node named `^root.field`; record its
    // spelling under its ledger id for the demand-sentence join.
    for leaf in &field_entries {
        resolver.name_step(leaf.id, PathSigil::Child, &leaf.name);
    }
    let built_indexes = resolver.build_indexes(
        &store.root.root,
        &key_entries,
        &field_entries,
        &store.indexes,
    );

    // Every identity must resolve before the graph enters the image; a single
    // gap already reported precisely leaves the durable graph absent, so an
    // operation over it is not additionally mislabelled "not yet executable"
    // (the identity gap is the diagnosis, whatever the shape). The placement name
    // is retained so a reference to `^name` steers to those identity reports
    // rather than reading as an unknown name.
    if let Some(refusal) = resolver.refusal.take() {
        return Ok(StoreBuild::Refused(refusal));
    }
    // The graph is admissible: its naming entries may now be committed with its root,
    // sites, and identity. A discarded (incomplete) graph never reaches here, so no
    // placeholder id enters the join.
    let naming = std::mem::take(&mut resolver.naming);
    // Admit the Product declaration now that every anchor in it resolved: the first store
    // over a resource binds it, and a later store reads back the very rows this one wrote.
    let members = match source {
        ProductDeclarationSource::Held(members) => members,
        ProductDeclarationSource::Built(commands) => {
            draft.declare_product(product, record.type_id, commands)?
        }
    };
    draft.set_application_identity(application);
    let key_columns: Vec<KeyColumn> = key_scalars
        .iter()
        .zip(&key_ids)
        .map(|(scalar, id)| KeyColumn {
            scalar: scalar.image(),
            id: *id,
        })
        .collect();

    // Admit the root occurrence over its Product declaration. It mints no site, and the
    // encoder sorts the string pool, so admitting the occurrence — and interning its
    // spelling — before the eager sites below leaves the wire unchanged while giving the
    // sites the occurrence they are qualified by.
    let indexes: Vec<DurableIndexShape> = built_indexes
        .iter()
        .map(|built| built.shape.clone())
        .collect();
    let root_name = draft.intern_string(&store.root.root);
    let admitted = draft.add_root_occurrence(
        product,
        RootOccurrenceDef {
            name: root_name,
            keys: key_columns,
            placement,
            indexes,
        },
    )?;
    let root_id = admitted.root_id();

    // Emit the eager, bounded per-node sites for the durable graph now: one
    // whole-payload site per keyed placement (this root and every nested `branch`)
    // and one whole-group site per static `group`. A site is named by the pair of the
    // root occurrence and the canonical declaration path of the node it addresses, so
    // the path owner projects the wire path from the same rows the member graph is
    // written from. The verifier re-derives every site from its own reconstructed node
    // set, so this claim is a producer claim, not a trusted address. Field-leaf sites are
    // NOT emitted here: they are the per-declared-field width driver, so the lowerer
    // binds and allocates (and deduplicates) one lazily on the first instruction that
    // addresses a field. The graph therefore captures each stored field's canonical
    // declaration path (below), and the site table scales with *referenced* fields, not
    // with declared width — an untouched field mints no site.
    request_eager_site(
        draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    )?;
    let captured = emit_root_member_sites(draft, admitted.occurrence(), &members)?;
    // One read site per managed index: a nonunique index is a progressive-prefix
    // scan, a unique index a complete-key exact lookup. There is deliberately no
    // index-write site — maintenance is compiler-owned. Every index site seals as
    // parked (an index node is never a flat-executable node); runtime traversal and
    // lookup land at E05.
    let mut lowered_indexes: Vec<DurableIndex> = Vec::with_capacity(built_indexes.len());
    for (built, path) in built_indexes.iter().zip(admitted.index_paths()) {
        let target = if built.shape.unique {
            SemanticTarget::IndexLookup
        } else {
            SemanticTarget::IndexScan
        };
        request_eager_site(draft, admitted.occurrence(), path, target)?;
        lowered_indexes.push(DurableIndex {
            name: built.name.clone(),
            unique: built.shape.unique,
            path: path.clone(),
            projection: built.projection.clone(),
        });
    }

    // Decide executability and capture the executable branch descriptors from the Product
    // declaration the draft now holds.
    //
    // Executable durable operations exist for the flat keyed root whose top-level fields
    // are each a scalar or a widened composite (a dense struct, or a closed
    // `enum`/`Option`/`Result` — framed inline in the field cell by the durable value
    // codec), together with its root-level groups of such fields and its field-only keyed
    // branches nested to any depth — the shape the kernel serves. A singleton (keyless)
    // root, a nominal field, or a group nested in a branch or another group parks
    // (severed until its lane lands): it carries its identity and full site set, but the
    // lowerer reports any operation over it as not yet executable. Composite root keys and
    // keyed branches (including composite-keyed) are executable for whole/field sites; a
    // root-level group no longer parks, mirroring the verifier's independent
    // `member_flat_at_root`.
    // `record.fields` (the registry record) carries only the top-level value fields;
    // its unkeyed groups live in `record.groups`, so a group value never appears here.
    let all_fields_executable = record
        .fields
        .iter()
        .all(|f| matches!(f.ty, GArg::Scalar(_) | GArg::Struct(_) | GArg::Enum(_)));
    // A keyed root of executable fields with root-level scalar/widened-field groups and
    // only field-only branches is executable, at any key arity (one or more columns); a
    // singleton root (no key columns) parks. `member_flat_at_root` admits a root-level
    // group of storable-value fields while `member_keeps_root_flat` (the branch-member
    // predicate) keeps a group parked below the root, so a group in a branch or another
    // group never makes the root flat — mirroring the verifier's `member_flat_at_root`.
    let keyed = !key_scalars.is_empty();
    let mut members_flat = true;
    for member in &members {
        members_flat &= member_flat_at_root(draft, member)?;
    }
    let executable = keyed && all_fields_executable && members_flat;
    let (branches, groups) = if executable {
        (
            build_executable_branches(records, resource, &captured.branches),
            build_executable_groups(&record.groups, &captured.groups),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    if !executable {
        return Ok(StoreBuild::Admitted(BuiltRoot {
            executable: None,
            naming,
        }));
    }
    // A flat root's top-level fields map positionally to the captured field paths, so
    // `captured.fields[i]` is the canonical declaration path of `record.fields[i]` (both
    // in member/record order). Each field carries its resolved value type (a scalar or a
    // widened composite), from which the lowerer builds the read/written value type; its
    // field-leaf site is bound and allocated lazily when an instruction first addresses
    // it.
    let fields = record
        .fields
        .iter()
        .zip(captured.fields)
        .map(|(field, path)| DurableField {
            name: field.name.clone(),
            path,
            ty: field.ty,
            required: field.required,
        })
        .collect();

    Ok(StoreBuild::Admitted(BuiltRoot {
        naming,
        executable: Some(DurableRoot {
            name: store.root.root.clone(),
            root_id,
            resource: store.resource.clone(),
            key: key_scalars.clone(),
            record: record.type_id,
            occurrence: admitted.occurrence().clone(),
            placement: admitted.placement_path().clone(),
            fields,
            groups,
            branches,
            indexes: lowered_indexes,
        }),
    }))
}

/// Resolve each key column's scalar in declared tuple order, rejecting a key type
/// outside the closed orderable durable-key set. A singleton placement has no columns
/// and yields an empty vector. Shared by root and branch key tuples.
///
/// The rejection row is returned rather than pushed: both callers refuse a store with
/// it, and a refusal is summarized from the row that reports it in one statement, so
/// the row cannot be spent here and the summary invented separately. It is boxed
/// because a diagnostic is wide next to a key-scalar vector and this path runs once
/// per refused store, never per admitted column.
fn resolve_key_scalars(
    file: &FileIdentity,
    span: SourceSpan,
    keys: &[KeyParam],
    records: &TypeRegistry,
) -> Result<Vec<ScalarType>, Box<SourceDiagnostic>> {
    let mut scalars = Vec::with_capacity(keys.len());
    for key_param in keys {
        let Some(key) = scalar_of(&records.expand(&key_param.ty)) else {
            return Err(Box::new(unsupported(file, span, "this key type")));
        };
        // The closed orderable durable-key scalar set (frozen at C04): int, string,
        // bool, bytes, date, and instant. `duration` is a span, not an identity, so
        // it is not a durable key.
        if !matches!(
            key,
            ScalarType::Int
                | ScalarType::Text
                | ScalarType::Bool
                | ScalarType::Bytes
                | ScalarType::Date
                | ScalarType::Instant
        ) {
            return Err(Box::new(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                span,
                "a durable key column must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                    .to_string(),
            )));
        }
        scalars.push(key);
    }
    Ok(scalars)
}

/// Resolves durable `(kind, path)` anchors against the committed ledger, pushing a
/// precise `check.durable_identity` diagnostic for each missing or retired anchor,
/// and building the group/branch member tree. `refusal` holds the first refusal of
/// this graph, if any; the caller discards the graph when it is set, so an id
/// resolved to a placeholder on a gap never reaches the image.
///
/// The refusal *is* the incompleteness — a graph with none is complete. The bare
/// flag this replaces recorded that a graph was refused while forgetting why, which
/// left every later `^root` reference to guess, and every one of them guessed
/// "identity gap".
struct IdentityResolver<'a> {
    declared: DeclarationSite<'a>,
    file: &'a FileIdentity,
    span: SourceSpan,
    ledger: Option<&'a IdentityLedger>,
    refusal: Option<DeclarationRefusalSummary>,
    /// The durable anchor spellings of enums whose sum/member anchors have already
    /// been resolved, so an enum reachable from several durable fields resolves —
    /// and reports any identity gap — exactly once.
    seen_enums: BTreeSet<String>,
    /// The first compiler-owned enum-shape coherence failure. It bypasses source
    /// diagnostics and aborts the durable build at the compile invariant boundary.
    invariant: Option<GenericInvariant>,
    /// The struct/enum value types on the current value-shape recursion path. It
    /// bounds the recursion by the finite distinct-type set: a type already on the
    /// path closes a cycle and short-circuits before the depth check. A cycle whose
    /// repeat falls within the depth bound is therefore pre-empted here and left to
    /// the later value-cycle `check.recursion` pass alone; a finite acyclic value, or a
    /// cycle whose distinct prefix first crosses the depth bound, reports its own
    /// `check.resource_limit` (the latter case then also draws `check.recursion` from
    /// the cycle pass — both are truthful and land at real spans).
    value_path: Vec<ValueNode>,
    /// The durable-path naming entries collected as this store's nodes resolve, drained
    /// into the [`BuiltRoot`] once the graph is known complete.
    naming: Vec<(LedgerIdBytes, PathSigil, String)>,
    /// Project-wide missing or retired anchors whose first resolution already emitted
    /// the typed gap. Every resolver still marks its own root incomplete on a shared
    /// gap; only diagnostic ownership is deduplicated.
    reported_identity_gaps: &'a mut BTreeSet<IdentityAnchor>,
    diagnostics: &'a mut DiagnosticCollector,
}

/// One struct or enum value type on the durable value-shape recursion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueNode {
    Struct(marrow_image::TypeId),
    Enum(marrow_image::EnumId),
}

impl<'a> IdentityResolver<'a> {
    fn new(
        declared: DeclarationSite<'a>,
        span: SourceSpan,
        ledger: Option<&'a IdentityLedger>,
        reported_identity_gaps: &'a mut BTreeSet<IdentityAnchor>,
        diagnostics: &'a mut DiagnosticCollector,
    ) -> Self {
        Self {
            declared,
            file: declared.file,
            span,
            ledger,
            refusal: None,
            seen_enums: BTreeSet::new(),
            invariant: None,
            value_path: Vec::new(),
            naming: Vec::new(),
            reported_identity_gaps,
            diagnostics,
        }
    }

    /// Refuse this durable graph, reporting `refusal` and keeping the summary of that
    /// report. The first refusal is the cause a later `^root` reference is steered to;
    /// a second one still reports at its own site but does not displace it, so the
    /// steer names the first thing that went wrong rather than the last.
    fn refuse(&mut self, refusal: DurableRefusal<'_>) {
        let summary = refuse_store(self.diagnostics, self.declared, refusal);
        if self.refusal.is_none() {
            self.refusal = Some(summary);
        }
    }

    /// Record one path-step node's source spelling under its ledger id, for the
    /// durable-path naming join. Only nodes a [`SemanticPath`] step names are recorded —
    /// a store root, a stored field, a managed index, a static group, or a keyed branch
    /// placement — never a key column or an enum sum/member, which are not path steps.
    fn name_step(&mut self, id: LedgerIdBytes, sigil: PathSigil, name: &str) {
        self.naming.push((id, sigil, name.to_string()));
    }

    /// Build a durable field's stored value shape from its resolved value type, over
    /// the closed acyclic durable value set. A nominal scalar erases to its base
    /// `int`; a dense struct records its leaves positionally with no per-leaf ledger
    /// id (the containing field is the renamable durable declaration); a closed enum
    /// resolves its sum (kind 5) and per-member (kind 6) identities. A collection or
    /// abstract type parameter is not a durable value leaf — it is a precise
    /// `check.unsupported` that marks the graph incomplete, so the placeholder shape
    /// is discarded with the graph.
    fn build_value_shape(
        &mut self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
        ty: GArg,
        depth: usize,
    ) -> DurableValueShape {
        match ty {
            // A leaf occupies a level of its own: the enclosing structs may all fit the
            // depth bound while the value that terminates them sits one level past it.
            // The image encoder measures the same shape and refuses it, so a leaf
            // admitted here without the bound would leave the compiler and the image
            // disagreeing about the same value — the source-level limit would be
            // reported as an internal invariant failure instead. The bound is checked at
            // the leaf for that reason, with the located diagnostic the enclosing
            // struct and enum arms already report.
            GArg::Scalar(scalar) => {
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                DurableValueShape::Scalar(scalar.image())
            }
            GArg::Nominal(_) => {
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                }
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
            GArg::Struct(type_id) => {
                // A struct already on the path closes a value cycle: leave it to the
                // later value-cycle pass (`check.recursion`) and drop the graph. The
                // cycle check precedes the depth check, so a cycle whose repeat falls
                // within the depth bound is pre-empted here and reported only by the
                // cycle pass. A finite acyclic value that reaches the depth bound is
                // genuinely over-deep and reports its own `check.resource_limit`; a
                // cycle whose distinct prefix crosses the depth bound first hits this
                // limit and additionally draws `check.recursion` — both truthful.
                if self.value_path.contains(&ValueNode::Struct(type_id)) {
                    self.refuse(DurableRefusal::ValueCycle);
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                match records.struct_by_type(type_id) {
                    Some(info) => {
                        if info.fields.len() > bounds::MAX_STRUCT_LEAVES {
                            self.reject_resource_limit(
                                self.span,
                                format!(
                                    "a durable struct value carries more than the fixed limit \
                                     of {} leaves",
                                    bounds::MAX_STRUCT_LEAVES
                                ),
                            );
                            return DurableValueShape::Scalar(ScalarType::Int.image());
                        }
                        self.value_path.push(ValueNode::Struct(type_id));
                        let leaves = info
                            .fields
                            .iter()
                            .map(|field| {
                                self.build_value_shape(records, metadata, field.ty, depth + 1)
                            })
                            .collect();
                        self.value_path.pop();
                        DurableValueShape::Struct(leaves)
                    }
                    None => {
                        self.reject_value("this struct value");
                        DurableValueShape::Struct(Vec::new())
                    }
                }
            }
            GArg::Enum(enum_id) => {
                if self.value_path.contains(&ValueNode::Enum(enum_id)) {
                    self.refuse(DurableRefusal::ValueCycle);
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                self.value_path.push(ValueNode::Enum(enum_id));
                let shape = self.build_enum_value_shape(records, metadata, enum_id, depth);
                self.value_path.pop();
                shape
            }
            GArg::Collection(_) => {
                self.reject_value(
                    "a collection stored directly in a durable field (a large collection \
                     belongs under a keyed branch)",
                );
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
            GArg::Group(_) => {
                // A group is a materialized-value namespace, never a durable top-level
                // field value (a durable group is its own member-tree node, resolved by
                // `build_extras`). It cannot reach here through `record.fields`.
                self.reject_value("a group stored directly as a durable field value");
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
            GArg::Param(_) => {
                self.reject_value("this value type");
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
        }
    }

    /// Build the value shape of a durable-reachable closed enum, resolving its sum
    /// and per-member ledger identities once (anchored at the enum's canonical
    /// spelling and `<spelling>.<member>`). Member order is declaration order, so
    /// append-only member evolution preserves every existing member's id and code.
    fn build_enum_value_shape(
        &mut self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
        enum_id: marrow_image::EnumId,
        depth: usize,
    ) -> DurableValueShape {
        let Some((variants, spelling)) = self.accept_ready_shape(
            metadata.durable_enum_shape_and_anchor(enum_id),
            "this enum value",
        ) else {
            return DurableValueShape::Scalar(ScalarType::Int.image());
        };
        // Resolve (and gap-report) an enum's anchors only the first time it is
        // reached; a later occurrence looks its ids up quietly.
        let first_time = match self.seen_enums.insert(spelling.clone()) {
            true => RefusalReport::AtDeclaration,
            false => RefusalReport::ByCoveringPass,
        };
        let sum = self.resolve_enum_anchor(IdentityKind::Sum, &spelling, first_time);
        let members = variants
            .iter()
            .map(|(name, payload)| {
                let id = self.resolve_enum_anchor(
                    IdentityKind::Member,
                    &format!("{spelling}.{name}"),
                    first_time,
                );
                let payload = payload
                    .iter()
                    .map(|arg| self.build_value_shape(records, metadata, *arg, depth + 1))
                    .collect();
                DurableEnumMemberShape { id, payload }
            })
            .collect();
        DurableValueShape::Enum { sum, members }
    }

    fn accept_ready_shape<T>(
        &mut self,
        result: Result<Option<T>, GenericInvariant>,
        subject: &str,
    ) -> Option<T> {
        match result {
            Ok(Some(value)) => Some(value),
            Ok(None) => {
                self.reject_value(subject);
                None
            }
            Err(invariant) => {
                self.remember_invariant(invariant);
                None
            }
        }
    }

    /// A compiler-owned coherence failure aborts the durable build at the invariant
    /// boundary — `build_one` returns it before the refusal gate is read — so it never
    /// becomes a refusal of the user's declaration.
    fn remember_invariant(&mut self, invariant: GenericInvariant) {
        if self.invariant.is_none() {
            self.invariant = Some(invariant);
        }
    }

    /// Resolve one enum sum/member anchor. On the enum's first occurrence this is the
    /// ordinary gap-reporting `resolve`; on a later occurrence it looks the id up
    /// quietly, since the first occurrence already reported any gap and discarded the
    /// graph.
    fn resolve_enum_anchor(
        &mut self,
        kind: IdentityKind,
        path: &str,
        report: RefusalReport,
    ) -> LedgerIdBytes {
        if matches!(report, RefusalReport::AtDeclaration) {
            return self.resolve(kind, path);
        }
        match self.ledger.and_then(|ledger| ledger.lookup(kind, path)) {
            Some(id) => LedgerIdBytes::from_bytes(*id.bytes()),
            None => LedgerIdBytes::from_bytes([0u8; 16]),
        }
    }

    /// Report a durable field value type outside the closed acyclic durable value set
    /// and mark the graph incomplete, so its placeholder value shape never reaches
    /// the image.
    fn reject_value(&mut self, subject: &str) {
        let span = self.span;
        self.refuse(DurableRefusal::Member { subject, span });
    }

    /// Report a durable construct that crosses a fixed compiler-owned resource bound
    /// at `span`, and mark the graph incomplete so its placeholder never reaches the
    /// image.
    fn reject_resource_limit(&mut self, span: SourceSpan, message: String) {
        self.refuse(DurableRefusal::Bound { message, span });
    }

    /// Resolve one anchor to its live ledger id. On a gap this reports the precise
    /// `(kind, path)` diagnostic, flips `complete` to false, and returns a
    /// placeholder id — the caller discards the whole graph when `complete` is
    /// false, so the placeholder is never encoded.
    fn resolve(&mut self, kind: IdentityKind, path: &str) -> LedgerIdBytes {
        if self.invariant.is_some() {
            return LedgerIdBytes::from_bytes([0u8; 16]);
        }
        let (live, retired) = match self.ledger {
            Some(ledger) => (ledger.lookup(kind, path), ledger.is_retired(kind, path)),
            None => (None, false),
        };
        match live {
            Some(id) => LedgerIdBytes::from_bytes(*id.bytes()),
            None => {
                let anchor = IdentityAnchor::new(kind, path);
                let report = match self.reported_identity_gaps.insert(anchor.clone()) {
                    true => RefusalReport::AtDeclaration,
                    false => RefusalReport::ByCoveringPass,
                };
                self.refuse(DurableRefusal::IdentityIncomplete {
                    anchor,
                    retired,
                    report,
                });
                LedgerIdBytes::from_bytes([0u8; 16])
            }
        }
    }

    /// The Product declaration's canonical member graph: the resource's top-level
    /// fields (aligned with the materialized record) followed by its static `group`
    /// namespaces and its keyed `branch` placements, each recursively holding its own
    /// members. A top-level field's value shape is drawn from the closed acyclic durable
    /// value set (a nominal scalar, a dense struct, a closed enum, or an `Option` of
    /// one), the field anchoring the ledger id while nested product leaves are shape
    /// bytes and each durable-reachable enum contributes its own sum/member identities.
    ///
    /// It is built once per Product, at that Product's first root in canonical
    /// store-traversal order: a later root over the same Product reads the declaration
    /// the draft already holds, resolving no anchor a second time and minting no second
    /// entry record type for the Product's nested branches.
    fn build_product_graph(
        &mut self,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
        store: &StoreDecl,
        resource: &ResourceDecl,
        record: &RecordInfo,
    ) -> Vec<DeclarationMemberDef> {
        let mut nodes: Vec<DeclarationDraftNode> = Vec::new();
        for field in &record.fields {
            let shape = DeclarationMemberShape::Field {
                id: self.resolve(
                    IdentityKind::Field,
                    &format!("{}.{}", store.resource, field.name),
                ),
                required: field.required,
                value: self.build_value_shape(records, metadata, field.ty, 1),
            };
            nodes.push(DeclarationDraftNode::declared(
                None,
                DeclarationWireClass::Field,
                shape,
            ));
        }
        // A member the type registry refused is still a member this resource declares,
        // so its identity anchor belongs to the resource's anchor set. It is resolved
        // here and contributes no node to the member tree, whose typed invariant stays
        // "built members only" — a refused member has no value shape to encode. Without
        // it the anchor set narrows exactly where a program is already wrong, and the
        // mint action that consumes these reports would write a ledger that is missing
        // the anchor the corrected program needs.
        for member in records.refused_members(&store.resource) {
            self.resolve(IdentityKind::Field, &format!("{}.{member}", store.resource));
        }
        self.build_extras(
            &mut nodes,
            None,
            draft,
            records,
            &resource.members,
            &store.resource,
        );
        declaration_commands(nodes)
    }

    /// Walk a resource's declared members in source order, appending one
    /// [`DeclarationDraftNode`] per static `group` namespace and keyed `branch` placement
    /// below `parent` — its stored fields are appended by the caller. `container` is the
    /// anchor path prefix — the resource name at the top level, extended by each group or
    /// branch name as the walk descends. A keyed scalar leaf or a non-scalar field inside
    /// a group or branch is a precise `check.unsupported` rejection.
    ///
    /// The walk is deliberately one pass in source order: identity resolution, string
    /// interning, and entry-record minting are order-sensitive side effects (a branch's
    /// entry record type is assigned by call order), so their sequence is fixed here while
    /// the wire order of the nodes is carried separately by each node's
    /// [`DeclarationWireClass`].
    fn build_extras(
        &mut self,
        nodes: &mut Vec<DeclarationDraftNode>,
        parent: Option<usize>,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        members: &[ResourceMember],
        container: &str,
    ) {
        for member in members {
            let ResourceMember::Group(group) = member else {
                continue;
            };
            let path = format!("{container}.{}", group.name);
            if group.keys.is_empty() {
                // A `group`: an unkeyed static field-path namespace. Its direct fields
                // flatten into the containing resource's namespace, so it mints no
                // record type of its own.
                let id = self.resolve(IdentityKind::Group, &path);
                self.name_step(id, PathSigil::Child, &group.name);
                let at = nodes.len();
                nodes.push(DeclarationDraftNode::declared(
                    parent,
                    DeclarationWireClass::Group,
                    DeclarationMemberShape::Group { id },
                ));
                self.build_member_tree(nodes, at, draft, records, group, &path);
            } else {
                // A keyed `branch`: a distinct keyed placement, like a root. Its entry
                // is a record of its own direct scalar fields; materialize that record
                // type (ordered like the member tree) so a whole branch-entry read
                // yields it and a create/replace supplies it. The record type name is
                // the qualified `Resource.branch` path — the branch's constructor
                // spelling; the branch's own `name` is the simple member name the
                // physical layer keys its family by.
                let placement = self.resolve(IdentityKind::Root, &path);
                self.name_step(placement, PathSigil::Child, &group.name);
                let keys = self.build_branch_keys(records, group, &path);
                // The branch's slot is reserved before its members are walked: its own
                // members declare it as their parent, and its entry record type is minted
                // from them, so the shape is completed once the walk returns.
                let at = nodes.len();
                nodes.push(DeclarationDraftNode::reserved(
                    parent,
                    DeclarationWireClass::Branch,
                ));
                let record_fields = self.build_member_tree(nodes, at, draft, records, group, &path);
                let record_name = draft.intern_string(&path);
                let record = draft.add_record_type(RecordTypeDef {
                    name: record_name,
                    fields: record_fields,
                });
                let name = draft.intern_string(&group.name);
                nodes[at].declare(DeclarationMemberShape::Branch {
                    placement,
                    name,
                    record,
                    keys,
                });
            }
        }
    }

    /// The key tuple of a branch placement: each column's scalar and its ledger id
    /// anchored at `<branch path>.<column>`. A key type outside the closed orderable
    /// durable-key set is a precise diagnostic and marks the graph incomplete.
    fn build_branch_keys(
        &mut self,
        records: &TypeRegistry,
        group: &GroupDecl,
        path: &str,
    ) -> Vec<KeyColumn> {
        if group.keys.len() > bounds::MAX_KEY_COLUMNS {
            self.reject_resource_limit(
                group.span,
                format!(
                    "a branch key tuple has {} columns; the fixed limit is {}",
                    group.keys.len(),
                    bounds::MAX_KEY_COLUMNS
                ),
            );
            return Vec::new();
        }
        let scalars = match resolve_key_scalars(self.file, group.span, &group.keys, records) {
            Ok(scalars) => scalars,
            Err(row) => {
                self.refuse(DurableRefusal::Admission { row: *row });
                return Vec::new();
            }
        };
        group
            .keys
            .iter()
            .zip(scalars)
            .map(|(key_param, scalar)| KeyColumn {
                scalar: scalar.image(),
                id: self.resolve(IdentityKind::Key, &format!("{path}.{}", key_param.name)),
            })
            .collect()
    }

    /// Append the members of one group or branch body below the node at `at`: its stored
    /// scalar fields, then its nested groups and branches. Field anchors are
    /// `<path>.<field>`. Returns the branch entry record's field layout, in the same order
    /// as the appended field nodes.
    fn build_member_tree(
        &mut self,
        nodes: &mut Vec<DeclarationDraftNode>,
        at: usize,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        group: &GroupDecl,
        path: &str,
    ) -> Vec<FieldDef> {
        let mut record_fields = Vec::new();
        for member in &group.members {
            let ResourceMember::Field(field) = member else {
                continue;
            };
            if let Some((shape, record_field)) = self.build_field(draft, records, field, path) {
                nodes.push(DeclarationDraftNode::declared(
                    Some(at),
                    DeclarationWireClass::Field,
                    shape,
                ));
                record_fields.push(record_field);
            }
        }
        self.build_extras(nodes, Some(at), draft, records, &group.members, path);
        record_fields
    }

    /// One stored scalar field of a group or branch: its ledger id, required flag,
    /// and scalar value shape. Group and branch leaves stay scalar-only on this line
    /// (top-level resource fields carry the widened value set); a keyed scalar leaf
    /// or a non-scalar group/branch field is a precise `check.unsupported` rejection
    /// and marks the graph incomplete.
    fn build_field(
        &mut self,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        field: &FieldDecl,
        container: &str,
    ) -> Option<(DeclarationMemberShape, FieldDef)> {
        if !field.keys.is_empty() {
            self.refuse(DurableRefusal::Member {
                subject: "a keyed field",
                span: field.span,
            });
            return None;
        }
        let Some(scalar) = scalar_of(&records.expand(&field.ty)) else {
            self.refuse(DurableRefusal::Member {
                subject: "a non-scalar field of a group or branch",
                span: field.span,
            });
            return None;
        };
        let id = self.resolve(IdentityKind::Field, &format!("{container}.{}", field.name));
        self.name_step(id, PathSigil::Child, &field.name);
        let member = DeclarationMemberShape::Field {
            id,
            required: field.required,
            value: DurableValueShape::Scalar(scalar.image()),
        };
        // The record field mirrors the durable member: same order, same scalar, same
        // required flag. The branch entry's whole-payload read/create/replace flows
        // through this record type.
        let record_field = FieldDef {
            name: draft.intern_string(&field.name),
            ty: ImageType::scalar(scalar.image()),
            required: field.required,
        };
        Some((member, record_field))
    }

    /// Resolve a root's managed indexes into their durable identity shapes, enforcing
    /// the closed narrow-index admission rules against the root's identity keys and
    /// top-level fields. A `store` index is either a nonunique ordered projection that
    /// must end with every identity key in declaration order (so each row is distinct)
    /// or a `unique` projection that may omit the identity keys. Every projected leaf
    /// must be one identity key or one top-level field whose stored value is an
    /// orderable durable-key scalar; a nested path, a name resolving to nothing, a
    /// group/branch or non-key-scalar leaf, a singleton root, or an index name
    /// colliding with a key/field/earlier index is a precise `check.type` rejection.
    /// Any violation marks the graph incomplete, so a rejected index discards the whole
    /// durable graph. The index's own `Index` ledger identity resolves through the
    /// ledger like every other durable anchor (a gap is `check.durable_identity`).
    fn build_indexes(
        &mut self,
        root: &str,
        keys: &[(String, LedgerIdBytes, ScalarType)],
        fields: &[IndexFieldLeaf],
        indexes: &[IndexDecl],
    ) -> Vec<BuiltIndex> {
        // The checker caps the per-root index count well below the image's structural
        // decode bound (`marrow_image::bounds::MAX_INDEXES`): each declared index is
        // compiler-maintained on every write to the root, so the cap bounds a root's write
        // amplification. The tighter checker limit is a product choice; the image bound
        // remains as headroom for a later increase without an image-format change.
        if indexes.len() > MAX_STORE_INDEXES {
            // The count itself is malformed, so report it and discard the graph rather than
            // validating and minting identities for indexes that cannot all be admitted.
            self.reject_index(
                indexes[MAX_STORE_INDEXES].span,
                format!(
                    "store root `{root}` declares {} managed indexes; at most \
                     {MAX_STORE_INDEXES} are allowed",
                    indexes.len()
                ),
            );
            return Vec::new();
        }
        let mut shapes = Vec::with_capacity(indexes.len());
        let mut seen_names: Vec<&str> = Vec::new();
        for index in indexes {
            // The projected component count crosses the fixed image projection width
            // before the index's leaves are resolved or its identity minted.
            if index.args.len() > bounds::MAX_INDEX_COMPONENTS {
                self.reject_resource_limit(
                    index.span,
                    format!(
                        "a managed index projects {} components; the fixed limit is {}",
                        index.args.len(),
                        bounds::MAX_INDEX_COMPONENTS
                    ),
                );
                continue;
            }
            // The index name shares the root's source namespace with the identity keys,
            // the stored fields, and the other indexes; a collision has no unambiguous
            // address.
            let name_collision = keys.iter().any(|(name, _, _)| name == &index.name)
                || fields.iter().any(|leaf| leaf.name == index.name)
                || seen_names.contains(&index.name.as_str());
            if name_collision {
                self.reject_index(
                    index.span,
                    format!(
                        "index `{}` collides with an identity key, a stored field, or another \
                         index of `{root}`",
                        index.name
                    ),
                );
                continue;
            }
            seen_names.push(&index.name);

            // An index entry points at one stored identity, so a singleton root (no
            // identity to point to) admits none.
            if keys.is_empty() {
                self.reject_index(
                    index.span,
                    format!("index `{}` requires a keyed store root", index.name),
                );
                continue;
            }

            let Some(resolved) = self.resolve_index_components(index, keys, fields) else {
                continue;
            };
            // The image identity references and lowerer-facing scalar projection are
            // two views of the same admitted components, in the same order.
            let components = resolved.iter().map(|item| item.component).collect();
            let projection = resolved.iter().map(|item| item.scalar).collect();
            let id = self.resolve(IdentityKind::Index, &format!("{root}.{}", index.name));
            self.name_step(id, PathSigil::Child, &index.name);
            shapes.push(BuiltIndex {
                shape: DurableIndexShape {
                    id,
                    unique: index.unique,
                    components,
                },
                name: index.name.clone(),
                projection,
            });
        }
        shapes
    }

    /// Resolve and validate one index's ordered projection into leaf references, or
    /// `None` (with a diagnostic and the graph marked incomplete) on any violation. A
    /// component resolves to an identity key or a top-level orderable-key field and
    /// appears at most once; a nonunique index must additionally end with every
    /// identity key in declaration order and carry no identity key in a leading
    /// position.
    fn resolve_index_components(
        &mut self,
        index: &IndexDecl,
        keys: &[(String, LedgerIdBytes, ScalarType)],
        fields: &[IndexFieldLeaf],
    ) -> Option<Vec<ResolvedIndexComponent>> {
        let mut components = Vec::with_capacity(index.args.len());
        let mut leading_key = false;
        let trailing_start = index.args.len().saturating_sub(keys.len());
        let mut ok = true;
        let mut seen_args: Vec<String> = Vec::with_capacity(index.args.len());
        for (position, component) in index.args.iter().enumerate() {
            let span = component.span;
            let arg = marrow_syntax::field_path_spelling(&component.segments);
            if seen_args.contains(&arg) {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` repeats component `{arg}`; each projection component appears \
                         at most once",
                        index.name
                    ),
                );
                ok = false;
                continue;
            }
            seen_args.push(arg.clone());
            // A path of more than one segment reaches through a member. The segments are
            // the path, so this asks the path rather than scanning a rendered spelling.
            if component.segments.len() > 1 {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` component `{arg}` reaches through a nested member; an index \
                         projects only top-level fields and identity keys",
                        index.name
                    ),
                );
                ok = false;
                continue;
            }
            let arg = arg.as_str();
            if let Some((_, key_id, scalar)) = keys.iter().find(|(name, _, _)| name == arg) {
                if !index.unique && position < trailing_start {
                    leading_key = true;
                }
                components.push(ResolvedIndexComponent {
                    component: DurableIndexComponent::Key(*key_id),
                    scalar: *scalar,
                });
            } else if let Some(leaf) = fields.iter().find(|leaf| leaf.name == arg) {
                let Some(scalar) = leaf.scalar else {
                    self.reject_index(
                        span,
                        format!(
                            "index `{}` component `{arg}` is not an orderable durable-key scalar",
                            index.name
                        ),
                    );
                    ok = false;
                    continue;
                };
                components.push(ResolvedIndexComponent {
                    component: DurableIndexComponent::Field(leaf.id),
                    scalar,
                });
            } else {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` component `{arg}` names no identity key or stored field of \
                         this root",
                        index.name
                    ),
                );
                ok = false;
            }
        }
        if !ok {
            return None;
        }
        // A nonunique index distinguishes rows by ending with the complete identity
        // suffix, in declaration order, with no identity key appearing earlier.
        if !index.unique {
            let ends_with_identity = index.args.len() >= keys.len()
                && keys.iter().enumerate().all(|(offset, (_, key_id, _))| {
                    matches!(
                        components.get(trailing_start + offset),
                        Some(ResolvedIndexComponent {
                            component: DurableIndexComponent::Key(id),
                            ..
                        }) if *id == *key_id
                    )
                });
            if leading_key || !ends_with_identity {
                self.reject_index(
                    index.span,
                    format!(
                        "non-unique index `{}` must end with the store's identity keys in \
                         declaration order",
                        index.name
                    ),
                );
                return None;
            }
        }
        Some(components)
    }

    /// Report a managed-index admission violation and mark the durable graph
    /// incomplete, so a rejected index discards the whole graph rather than emitting a
    /// partial one.
    fn reject_index(&mut self, span: SourceSpan, message: String) {
        self.refuse(DurableRefusal::Index { message, span });
    }
}

/// The wire order of one declaration level: stored fields, then static `group`
/// namespaces, then keyed `branch` placements, each in source order within its class.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeclarationWireClass {
    Field,
    Group,
    Branch,
}

/// One node of a Product declaration as the source-order traversal found it: the buffer
/// position of the node it nests under, the wire class that orders it among its siblings,
/// and its declared shape.
///
/// The traversal order and the wire order deliberately differ. Identity resolution, string
/// interning, and entry-record minting are order-sensitive side effects — a branch's entry
/// record type is assigned by call order — so the walk runs once in source order and its
/// buffer position *is* the traversal sequence, while `class` carries where the node
/// belongs on the wire. [`declaration_commands`] projects the two into the flat command
/// vector the draft admits.
struct DeclarationDraftNode {
    parent: Option<usize>,
    class: DeclarationWireClass,
    /// Vacant only while a keyed branch's own members are being walked: the branch
    /// declares its slot first so its members can name it as their parent, and its entry
    /// record type is minted from those members.
    shape: Option<DeclarationMemberShape>,
}

impl DeclarationDraftNode {
    fn declared(
        parent: Option<usize>,
        class: DeclarationWireClass,
        shape: DeclarationMemberShape,
    ) -> Self {
        Self {
            parent,
            class,
            shape: Some(shape),
        }
    }

    fn reserved(parent: Option<usize>, class: DeclarationWireClass) -> Self {
        Self {
            parent,
            class,
            shape: None,
        }
    }

    fn declare(&mut self, shape: DeclarationMemberShape) {
        self.shape = Some(shape);
    }
}

/// Project the source-order traversal buffer into the flat command vector
/// [`ImageDraft::declare_product`] admits: every parent before its children, and each
/// parent's children in wire order — stored fields, then static groups, then keyed
/// branches, each in the order the traversal met them.
///
/// The draft places a command vector's rows level by level and keeps each parent's
/// commands in the order they arrived, so ordering the children here is what fixes the
/// declaration's wire order.
fn declaration_commands(mut nodes: Vec<DeclarationDraftNode>) -> Vec<DeclarationMemberDef> {
    // Bucket 0 holds the Product's own direct members; bucket `n + 1` holds node `n`'s.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len() + 1];
    for (at, node) in nodes.iter().enumerate() {
        children[node.parent.map_or(0, |parent| parent + 1)].push(at);
    }
    for bucket in &mut children {
        bucket.sort_by_key(|at| (nodes[*at].class, *at));
    }
    let mut commands = Vec::with_capacity(nodes.len());
    emit_declaration_commands(&mut commands, &mut nodes, &children, 0, None);
    commands
}

/// The most commands [`declaration_commands`] emits: one past the member bound, which is
/// exactly what the draft needs to record the overflow and the encoder needs to refuse the
/// declaration. Emitting the whole of an over-wide declaration would let a parent index
/// pass `u16`, which the command form cannot spell.
const MAX_DECLARATION_COMMANDS: usize = bounds::MAX_DURABLE_MEMBERS + 1;

/// Emit the already-ordered children in `bucket` and, immediately after each, that node's
/// own children — so a parent's command index is always strictly less than its children's,
/// which is the one shape the command form admits.
fn emit_declaration_commands(
    commands: &mut Vec<DeclarationMemberDef>,
    nodes: &mut [DeclarationDraftNode],
    children: &[Vec<usize>],
    bucket: usize,
    parent: Option<u16>,
) {
    for at in &children[bucket] {
        if commands.len() >= MAX_DECLARATION_COMMANDS {
            return;
        }
        let Some(shape) = nodes[*at].shape.take() else {
            continue;
        };
        #[expect(
            clippy::expect_used,
            reason = "bounded projection: the emission stops at MAX_DECLARATION_COMMANDS, which is far below u16::MAX"
        )]
        let command = u16::try_from(commands.len()).expect("the command count is bounded");
        commands.push(DeclarationMemberDef { parent, shape });
        emit_declaration_commands(commands, nodes, children, at + 1, Some(command));
    }
}

/// The canonical declaration paths and materialized record of one keyed branch: the
/// branch's own path, its direct fields' paths in declaration order, its entry record
/// type, and the same for each of its nested branches. For an executable branch these back
/// the branch's whole-entry operations and its field-exact
/// `^root(k).branch(bk).field` operations respectively; a non-flat root parks them and
/// consumes neither.
struct BranchSites {
    path: CanonicalDeclarationPathSelector,
    fields: Vec<CanonicalDeclarationPathSelector>,
    record: marrow_image::TypeId,
    /// This branch's own nested branches, in declaration order, so a nested-branch lowerer
    /// resolves a deeper `^root(k).b(bk).s(sk)` path level by level.
    branches: Vec<BranchSites>,
}

/// What the root's member walk captures for the executable lowerer: the canonical paths of
/// the root's direct stored fields and root-level groups, each in declaration order, and
/// one capture per top-level keyed branch.
struct RootMemberSites {
    fields: Vec<CanonicalDeclarationPathSelector>,
    groups: Vec<CanonicalDeclarationPathSelector>,
    branches: Vec<BranchSites>,
}

/// Bind one eager site over `occurrence` at `path` and request it, so its id is minted in
/// this call's position. The operand is not retained: a descriptor holds the selector, and
/// the instruction that names the site re-binds and re-requests it, which returns the id
/// minted here.
fn request_eager_site(
    draft: &mut ImageDraft,
    occurrence: &RootOccurrenceSelector,
    path: &CanonicalDeclarationPathSelector,
    target: SemanticTarget,
) -> Result<(), GenericInvariant> {
    let handle = draft.bind_occurrence_site(occurrence, path, target)?;
    draft.request_site(&handle)?;
    Ok(())
}

/// Emit the eager (bounded, per-node) operation sites of the root's member graph and
/// capture what the flat executable lowerer needs: each root-level group's canonical path
/// (in declaration order), each top-level branch's path and record (recursively), and the
/// canonical paths of the root's direct fields. Field-leaf sites are not emitted here —
/// the lowerer binds and allocates one lazily on first reference — so a wide resource's
/// site table scales with referenced fields, not declared width. A group is a namespace
/// whose leaves are addressed through its whole-group site, so no per-leaf site is emitted.
/// The eager sites are emitted pre-order, a placement or group node before its members,
/// mirroring [`marrow_image::DurableContractDescriptor::semantic_nodes`] so every emitted
/// site resolves against the verifier's independently reconstructed node set.
fn emit_root_member_sites(
    draft: &mut ImageDraft,
    occurrence: &RootOccurrenceSelector,
    members: &[DeclarationMember],
) -> Result<RootMemberSites, GenericInvariant> {
    let mut captured = RootMemberSites {
        fields: Vec::new(),
        groups: Vec::new(),
        branches: Vec::new(),
    };
    for member in members {
        match member.shape() {
            DeclarationMemberShape::Field { .. } => captured.fields.push(member.path().clone()),
            DeclarationMemberShape::Group { .. } => {
                request_eager_site(draft, occurrence, member.path(), SemanticTarget::GroupEntry)?;
                captured.groups.push(member.path().clone());
            }
            DeclarationMemberShape::Branch { record, .. } => {
                let record = *record;
                captured.branches.push(emit_branch_sites(
                    draft,
                    occurrence,
                    member.path(),
                    record,
                )?);
            }
        }
    }
    Ok(captured)
}

/// Emit one keyed branch's eager whole-payload entry site and capture it recursively with
/// its direct fields' canonical paths (leaf sites allocated lazily on reference) and each
/// nested branch's capture. A static `group` inside a branch parks the whole root
/// (`member_keeps_root_flat` refuses it), so on the executable path only fields and nested
/// branches occur. The direct field order is the branch's materialized-record order — the
/// leaf the verifier seals as `BranchField(field)` — and the nested-branch order indexes
/// the sealed branch tree, so the compiler's and verifier's independent resolutions agree.
fn emit_branch_sites(
    draft: &mut ImageDraft,
    occurrence: &RootOccurrenceSelector,
    path: &CanonicalDeclarationPathSelector,
    record: marrow_image::TypeId,
) -> Result<BranchSites, GenericInvariant> {
    request_eager_site(draft, occurrence, path, SemanticTarget::WholePayload)?;
    let members = draft.members_of(path)?;
    let mut fields = Vec::new();
    let mut branches = Vec::new();
    for inner in &members {
        match inner.shape() {
            DeclarationMemberShape::Field { .. } => fields.push(inner.path().clone()),
            DeclarationMemberShape::Group { .. } => {}
            DeclarationMemberShape::Branch { record, .. } => {
                let record = *record;
                branches.push(emit_branch_sites(draft, occurrence, inner.path(), record)?);
            }
        }
    }
    Ok(BranchSites {
        path: path.clone(),
        fields,
        record,
        branches,
    })
}

/// Whether a durable member keeps its containing root flat-executable, mirroring the
/// verifier's independent `keeps_root_flat`: a field (scalar or widened struct/enum — the
/// durable field codec frames a composite inline in its cell), or a field-only keyed
/// branch (one or more key columns) whose direct members recursively keep flat. A static
/// `group` does not. (A `Field`'s value shape is always a scalar, struct, or enum — a
/// collection field is rejected upstream — so any field keeps the root flat here.)
fn member_keeps_root_flat(
    draft: &ImageDraft,
    member: &DeclarationMember,
) -> Result<bool, GenericInvariant> {
    match member.shape() {
        DeclarationMemberShape::Field { value, .. } => Ok(matches!(
            value,
            DurableValueShape::Scalar(_)
                | DurableValueShape::Struct(_)
                | DurableValueShape::Enum { .. }
        )),
        DeclarationMemberShape::Group { .. } => Ok(false),
        DeclarationMemberShape::Branch { keys, .. } => {
            if keys.is_empty() {
                return Ok(false);
            }
            for inner in &draft.members_of(member.path())? {
                if !member_keeps_root_flat(draft, inner)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

/// Whether a root's *direct* member keeps the root flat-executable, mirroring the
/// verifier's independent `member_flat_at_root`. It admits one more shape than
/// [`member_keeps_root_flat`]: a root-level unkeyed `group` whose own members are all
/// storable-value fields (a scalar or a widened composite). A group is a value unit of the
/// root entry, executable at the root level; a group nested in a branch or in another
/// group still parks, because branch members are classified by [`member_keeps_root_flat`]
/// (which keeps `Group => false`), so a group below the root's direct members never makes
/// its enclosing branch flat.
fn member_flat_at_root(
    draft: &ImageDraft,
    member: &DeclarationMember,
) -> Result<bool, GenericInvariant> {
    match member.shape() {
        DeclarationMemberShape::Field { .. } | DeclarationMemberShape::Branch { .. } => {
            member_keeps_root_flat(draft, member)
        }
        DeclarationMemberShape::Group { .. } => Ok(draft
            .members_of(member.path())?
            .iter()
            .all(|inner| matches!(inner.shape(), DeclarationMemberShape::Field { .. }))),
    }
}

/// The executable root-level group descriptors of a flat-executable root, in declaration
/// order. Each group's materialized record and its scalar/widened leaves come from the
/// registry `GroupInfo` (`groups`), and its canonical declaration path from `paths` — both
/// in the same declaration order, so a group descriptor and its path align by position.
/// Called only when the caller has proven the root flat-executable, so every group is a
/// storable-value-field group.
fn build_executable_groups(
    groups: &[crate::types::GroupInfo],
    paths: &[CanonicalDeclarationPathSelector],
) -> Vec<DurableGroup> {
    groups
        .iter()
        .zip(paths)
        .map(|(group, path)| DurableGroup {
            name: group.name.clone(),
            record: group.type_id,
            path: path.clone(),
            fields: group
                .fields
                .iter()
                .map(|leaf| DurableGroupLeaf {
                    name: leaf.name.clone(),
                    ty: leaf.ty,
                    required: leaf.required,
                })
                .collect(),
        })
        .collect()
}

/// The executable branch descriptors of a flat-executable root, in declaration order,
/// recursively. Each branch's materialized record type and its whole-payload, per-field,
/// and nested-branch sites come from `top_branches`, and its simple name, key, field plan,
/// and nested branches from the source resource declaration — all in the same declaration
/// order, so a branch path indexes both the sealed branch tree and this one identically.
/// Called only when the caller has proven the root flat-executable, so every branch is a
/// scalar-field keyed branch (its nested members are scalar fields and simple
/// branches).
fn build_executable_branches(
    records: &TypeRegistry,
    resource: &ResourceDecl,
    top_branches: &[BranchSites],
) -> Vec<DurableBranch> {
    build_branches(records, &resource.members, top_branches)
}

/// Build the [`DurableBranch`] descriptors for the keyed branches among `members`, zipped
/// positionally against their captured `sites`, recursing into each branch's own members
/// and captured nested-branch sites. The source keyed groups and the captured `BranchSites`
/// are both in declaration order, so the zip pairs each branch with its own sites.
fn build_branches(
    records: &TypeRegistry,
    members: &[ResourceMember],
    sites: &[BranchSites],
) -> Vec<DurableBranch> {
    members
        .iter()
        .filter_map(|member| match member {
            ResourceMember::Group(group) if !group.keys.is_empty() => Some(group),
            _ => None,
        })
        .zip(sites)
        .map(|(group, sites)| {
            #[expect(
                clippy::expect_used,
                reason = "checker-classified type: key columns admitted to an executable branch were classified as orderable key scalars during checking, so expansion yields a scalar"
            )]
            let key = group
                .keys
                .iter()
                .map(|column| {
                    scalar_of(&records.expand(&column.ty))
                        .expect("an executable branch key column is an orderable key scalar")
                })
                .collect();
            let fields = group
                .members
                .iter()
                .filter_map(|member| match member {
                    ResourceMember::Field(field) => Some(field),
                    _ => None,
                })
                .zip(&sites.fields)
                .map(|(field, path)| {
                    #[expect(
                        clippy::expect_used,
                        reason = "checker-classified type: fields admitted to an executable branch were classified as scalars during checking, so expansion yields a scalar"
                    )]
                    let scalar = scalar_of(&records.expand(&field.ty))
                        .expect("an executable branch field is a scalar");
                    DurableBranchField {
                        name: field.name.clone(),
                        scalar,
                        required: field.required,
                        path: path.clone(),
                    }
                })
                .collect();
            let branches = build_branches(records, &group.members, &sites.branches);
            DurableBranch {
                name: group.name.clone(),
                key,
                record: sites.record,
                path: sites.path.clone(),
                fields,
                branches,
            }
        })
        .collect()
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

/// The precise missing/retired-identity diagnostic: the typed `(kind, path)`
/// gap plus a message naming the identity and the command that mints it.
fn identity_gap(
    file: &FileIdentity,
    span: SourceSpan,
    kind: IdentityKind,
    path: &str,
    retired: bool,
) -> SourceDiagnostic {
    let message = if retired {
        format!(
            "durable identity for {} `{}` was retired in .marrow/ids and can never be reused; \
             declare a fresh name",
            kind.keyword(),
            path
        )
    } else {
        format!(
            "durable identity for {} `{}` is missing from .marrow/ids; \
             `marrow run` mints missing identities (commit the updated .marrow/ids)",
            kind.keyword(),
            path
        )
    };
    SourceDiagnostic::with_identity_gap(
        Code::CheckDurableIdentity.as_str(),
        file,
        span,
        message,
        IdentityGap {
            kind,
            path: path.to_string(),
            retired,
        },
    )
}

fn unsupported(file: &FileIdentity, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

/// A `check.resource_limit`: one durable construct crosses a fixed compiler-owned
/// bound the image cannot represent, reported at the offending construct's span so
/// the source, not a fabricated location, carries the diagnostic.
fn resource_limit(file: &FileIdentity, span: SourceSpan, message: String) -> SourceDiagnostic {
    SourceDiagnostic::at(Code::CheckResourceLimit.as_str(), file, span, message)
}

fn over_deep_value_message() -> String {
    format!(
        "a durable field value nests structs or enums deeper than the fixed limit of {} levels",
        bounds::MAX_DURABLE_VALUE_DEPTH
    )
}

#[cfg(test)]
mod generic_enum_shape_tests {
    use super::*;
    use crate::types::{MintSite, TypeInstId, TypeInstKind};
    use marrow_syntax::{Declaration, parse_source};

    /// The store declaration these resolvers refuse against. The resolver retains its
    /// refusal under the declared placement name, so it needs the declaration's
    /// coordinates even when the test drives only one value-shape walk.
    fn test_declared() -> DeclarationSite<'static> {
        DeclarationSite {
            name: "probe",
            file: crate::test_main_file_identity(),
            at: FileRef::admitted(0),
            span: SourceSpan::default(),
        }
    }

    /// A committed reserved enum reaches the durable-shape owner
    /// with its exact member and payload layout. Missing ledger rows may make the
    /// enclosing graph incomplete, but do not turn a Ready enum into an unavailable
    /// generic row.
    #[test]
    fn ready_option_reaches_the_durable_enum_shape_owner() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(build_diagnostics.is_empty());
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: SourceSpan {
                        line: 1,
                        column: 1,
                        ..SourceSpan::default()
                    },
                },
            )
            .expect("Ready Option mints");

        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );
        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(
                    resolver.build_enum_value_shape(&records, metadata, option, 0),
                )
            })
            .expect("the Ready Option metadata session opens");
        let DurableValueShape::Enum { members, .. } = shape else {
            panic!("a Ready Option remains enum-shaped")
        };
        assert_eq!(members.len(), 2);
        assert!(members[0].payload.is_empty());
        assert_eq!(members[1].payload.len(), 1);
        assert_eq!(
            members[1].payload[0],
            DurableValueShape::Scalar(ScalarType::Int.image())
        );
        assert!(resolver.seen_enums.contains("Option[int]"));
        assert!(
            resolver.refusal.is_some(),
            "the test intentionally supplies no ledger"
        );
        drop(resolver);
        assert_eq!(
            diagnostics.probe_rows().len(),
            3,
            "sum plus two member identity gaps"
        );
        assert!(
            diagnostics
                .probe_rows()
                .iter()
                .all(|diagnostic| diagnostic.code() == Code::CheckDurableIdentity.as_str())
        );
    }

    /// An image enum with no Ready semantic row is refused before
    /// durable identity spelling or member resolution can observe it.
    #[test]
    fn unavailable_enum_stops_before_durable_identity_resolution() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(build_diagnostics.is_empty());
        let name = draft.intern_string("Unavailable");
        let unavailable = draft.add_enum_type(marrow_image::EnumTypeDef {
            name,
            variants: Vec::new(),
        });
        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );

        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(resolver.build_enum_value_shape(
                    &records,
                    metadata,
                    unavailable,
                    0,
                ))
            })
            .expect("the unavailable enum metadata session opens");
        assert_eq!(shape, DurableValueShape::Scalar(ScalarType::Int.image()));
        assert!(resolver.refusal.is_some());
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert_eq!(diagnostics.probe_rows().len(), 1);
        assert_eq!(
            diagnostics.probe_rows()[0].code(),
            Code::CheckUnsupported.as_str()
        );
        assert!(diagnostics.probe_rows()[0].identity_gap().is_none());
    }

    #[test]
    fn ready_enum_with_struct_body_is_not_contextualized_or_resolved() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: SourceSpan::default(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );

        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this enum value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn durable_typed_error_stops_before_identity_or_draft_effects() {
        let parsed = parse_source(
            r#"resource Holder {
    required value: Option<int>
}

store ^holders[id: int]: Holder
"#,
        );
        assert!(!parsed.has_errors());
        let resource = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) => Some(resource),
                _ => None,
            })
            .expect("resource parses");
        let resources = vec![(
            FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            resource,
        )];
        let mut draft = ImageDraft::new();
        let mut diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &resources,
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(diagnostics.is_empty());
        let option = match records.by_name("Holder").expect("record exists").fields[0].ty {
            GArg::Enum(id) => id,
            _ => panic!("resource field resolves to Option"),
        };
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );
        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this durable value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    /// The projection is appended in the same statement as the ledger entry, so a
    /// resource naming a placement the ledger does not know is the two having drifted.
    /// Answering `Absent` there would put a fabricated absence back at the use site — the
    /// defect this projection exists to remove — reached through the projection instead of
    /// through the executable list.
    #[test]
    fn a_projection_naming_an_unknown_placement_is_drift_not_absence() {
        let mut registry = DurableRegistry::empty(DeclarationBudget::default());
        registry.products.insert(
            "Holder".to_string(),
            ProductStores {
                admitted: vec!["holders".to_string()],
                first_refused: None,
                declared_branches: false,
            },
        );
        assert!(matches!(
            registry.product("Holder"),
            Err(DeclarationIndexDrift)
        ));
        // A resource whose every store was refused steers to the first cause; a
        // projection recording neither an admitted nor a refused store is incoherent.
        registry.products.insert(
            "Neither".to_string(),
            ProductStores {
                admitted: Vec::new(),
                first_refused: None,
                declared_branches: false,
            },
        );
        assert!(matches!(
            registry.product("Neither"),
            Err(DeclarationIndexDrift)
        ));
        // A resource no store binds has no projection entry at all, which is the
        // genuine absence and stays one.
        assert!(matches!(
            registry.product("Unbound"),
            Ok(ProductBinding::Absent)
        ));
    }
}

#[cfg(test)]
mod declaration_command_bound_tests {
    use super::*;
    use marrow_image::{ExportId, FunctionDef, ImageBuildError, Instr, SpanEntry};

    const APPLICATION_ID: [u8; 16] = [0x0a; 16];
    const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
    const KEY_ID: [u8; 16] = [0x0c; 16];
    const PRODUCT_ID: [u8; 16] = [0x0d; 16];

    /// A distinct member ledger id seeded by `n`, so a width fixture cannot be
    /// answered by an identity-collision refusal instead of the bound under test.
    fn member_id(n: usize) -> LedgerIdBytes {
        let mut bytes = [0x40u8; 16];
        bytes[0] = n as u8;
        bytes[1] = (n >> 8) as u8;
        LedgerIdBytes::from_bytes(bytes)
    }

    /// Encode a minimal image whose one keyed root projects a Product declaring exactly
    /// `commands`, so the declaration width is the only reason an encode can fail.
    fn encode_product(commands: Vec<DeclarationMemberDef>) -> Result<(), ImageBuildError> {
        let mut draft = ImageDraft::new();
        let type_name = draft.intern_string("R");
        let record = draft.add_record_type(RecordTypeDef {
            name: type_name,
            fields: Vec::new(),
        });
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        let root_name = draft.intern_string("r");
        draft
            .declare_product(LedgerIdBytes::from_bytes(PRODUCT_ID), record, commands)
            .expect("a well-formed flat declaration");
        draft
            .add_root_occurrence(
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name: root_name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: LedgerIdBytes::from_bytes(KEY_ID),
                    }],
                    placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
        let src = draft.intern_string("src/main.mw");
        let main_name = draft.intern_string("main");
        let zero = draft.intern_int(0);
        let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
        let spans = (0..code.len())
            .map(|index| SpanEntry {
                instr_index: index as u32,
                line: 1,
                column: 1,
            })
            .collect();
        let main = draft
            .add_function(FunctionDef {
                name: main_name,
                source: src,
                params: Vec::new(),
                ret: ImageType::scalar(Scalar::Int),
                local_count: 0,
                code,
                spans,
            })
            .expect("every site operand is live");
        draft.add_export(ExportId::of_local("", "main"), main);
        draft.encode().map(|_| ())
    }

    /// The declaration member bound has two owners and they agree by exactly one command.
    /// This module stops emitting at [`MAX_DECLARATION_COMMANDS`]; `marrow-image` records
    /// a declaration as over-bound only at *more* than `MAX_DURABLE_MEMBERS` rows and the
    /// encoder then refuses the image with
    /// [`ImageBuildError::TooManyDurableMembers`] — which the compiler classifies as the
    /// `DurableMembers` resource limit. Truncating one command lower would hand the image
    /// owner a full-width declaration it accepts, and the over-wide resource would encode
    /// silently short instead of being refused. This drives the real emitter with an
    /// over-wide node buffer and carries its output to a real encode, so moving either
    /// bound without the other fails here.
    ///
    /// It is a producer-seam fixture rather than a `compile()`-tier one because the width
    /// is not reachable from source today: every member anchors one identity ledger row,
    /// and `marrow-project`'s `MAX_IDS_ROWS` (8192) admits no ledger that also carries the
    /// application, product, placement, and key rows a resource of this width needs.
    #[test]
    fn one_member_past_the_member_bound_encodes_as_too_many_durable_members() {
        assert_eq!(
            MAX_DECLARATION_COMMANDS,
            bounds::MAX_DURABLE_MEMBERS + 1,
            "the emission cap must sit exactly one command past the image owner's bound"
        );

        let nodes: Vec<DeclarationDraftNode> = (0..bounds::MAX_DURABLE_MEMBERS + 64)
            .map(|n| {
                DeclarationDraftNode::declared(
                    None,
                    DeclarationWireClass::Field,
                    DeclarationMemberShape::Field {
                        id: member_id(n),
                        required: false,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    },
                )
            })
            .collect();
        let commands = declaration_commands(nodes);
        assert_eq!(
            commands.len(),
            bounds::MAX_DURABLE_MEMBERS + 1,
            "an over-wide resource emits exactly one command past the member bound"
        );

        assert!(
            matches!(
                encode_product(commands),
                Err(ImageBuildError::TooManyDurableMembers)
            ),
            "one command past the bound must reach the encoder as the durable-member limit"
        );

        let at_bound = declaration_commands(
            (0..bounds::MAX_DURABLE_MEMBERS)
                .map(|n| {
                    DeclarationDraftNode::declared(
                        None,
                        DeclarationWireClass::Field,
                        DeclarationMemberShape::Field {
                            id: member_id(n),
                            required: false,
                            value: DurableValueShape::Scalar(Scalar::Int),
                        },
                    )
                })
                .collect(),
        );
        assert_eq!(at_bound.len(), bounds::MAX_DURABLE_MEMBERS);
        assert!(
            encode_product(at_bound).is_ok(),
            "a declaration exactly at the member bound still encodes"
        );
    }
}
