//! The declaration-side row tables the durable build reads instead of syntax.
//!
//! Each table is taken once, before the first store is built, from the two owners a
//! durable declaration has: the declaration the parser wrote, and the fact the type
//! registry admitted for it. The build then reaches a declaration only through a row,
//! so a consumer cannot re-derive an answer this module already decided, and the two
//! owners disagreeing about one name is unrepresentable rather than reported.

use std::collections::BTreeMap;
use std::ops::Range;

use marrow_image::bounds;
use marrow_project::FileIdentity;
use marrow_syntax::{IndexDecl, KeyParam, ResourceDecl, SourceSpan, StoreDecl, TypeExpr};

use crate::analysis::FileRef;
use crate::types::{GenericInvariant, RecordInfo, TypeRegistry};
/// A typed handle to one admitted `resource` declaration, valid only in the
/// [`ResourceDirectory`] that minted it.
///
/// A row index rather than a narrowed ordinal: it addresses this compile's
/// declaration list and never reaches the wire.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResourceDeclId(usize);

/// One `resource` declaration as the durable builder reads it: the declaration the
/// parser wrote, and the record the type registry admitted for it.
pub(super) struct ResourceRow<'a> {
    pub(super) decl: &'a ResourceDecl,
    pub(super) record: &'a RecordInfo,
}

/// Every `resource` declaration the type registry admitted, addressed by
/// [`ResourceDeclId`] and looked up by written spelling.
///
/// This is the one join between a resource's two owners — the declaration the parser
/// wrote and the record the type registry admitted — and it is performed once, before
/// any store is built. The registry drives the join: every row comes from an admitted
/// record, so which resources exist is decided by exactly one owner, and a store
/// reaches a record only *through* a row that already holds the declaration it was
/// built from. An admitted record whose declaration is missing from the received
/// slice is the two build inputs drifting apart — a compiler coherence failure raised
/// here, at the single join, and nowhere else.
pub(super) struct ResourceDirectory<'a> {
    rows: Vec<ResourceRow<'a>>,
    by_spelling: BTreeMap<&'a str, ResourceDeclId>,
}

impl<'a> ResourceDirectory<'a> {
    pub(super) fn take(
        resources: &[(FileRef, FileIdentity, &'a ResourceDecl)],
        records: &'a TypeRegistry,
    ) -> Result<Self, GenericInvariant> {
        // A repeated resource name is refused by the declare pass, which keeps the
        // first declaration; answering the spelling with the first declaration is the
        // same choice, spelled once. One keyed pass here; the join below is a lookup,
        // not a scan, so the take is linear-logarithmic in the declaration count.
        let mut declared: BTreeMap<&str, &'a ResourceDecl> = BTreeMap::new();
        for (_, _, decl) in resources {
            declared.entry(decl.name.as_str()).or_insert(decl);
        }
        let mut rows = Vec::new();
        let mut by_spelling = BTreeMap::new();
        for record in records.admitted_resources() {
            let Some(decl) = declared.get(record.name.as_str()) else {
                return Err(GenericInvariant::DurableResourceMissing(record.type_id));
            };
            let id = ResourceDeclId(rows.len());
            rows.push(ResourceRow { decl, record });
            by_spelling.insert(record.name.as_str(), id);
        }
        Ok(Self { rows, by_spelling })
    }

    fn lookup(&self, spelling: &str) -> Option<ResourceDeclId> {
        self.by_spelling.get(spelling).copied()
    }

    /// The row `id` addresses. `id` is minted only by [`Self::take`], from a length
    /// taken immediately before the matching push, so it addresses a row of this
    /// directory by construction.
    pub(super) fn row(&self, id: ResourceDeclId) -> &ResourceRow<'a> {
        &self.rows[id.0]
    }
}

/// One `store` declaration's resource binding, resolved before any store is built.
///
/// The row carries the written spelling beside the binding because every diagnostic
/// the binding produces renders that spelling: a row that held only the resolution
/// would send its consumer back to the declaration for the half it reports.
pub(super) struct StoreRow<'a> {
    pub(super) resource: &'a str,
    pub(super) binding: StoreResourceBinding,
    /// The root's managed indexes, taken from the declaration with the binding so the
    /// build reads no `index` syntax of its own.
    pub(super) indexes: IndexTable<'a>,
    /// The root's identity key tuple, taken with the same reading.
    pub(super) keys: KeyTable<'a>,
}

/// What a `store` declaration's written resource spelling binds to.
pub(super) enum StoreResourceBinding {
    /// The spelling names a `resource` declaration the type registry admitted.
    Accepted(ResourceDeclId),
    /// No admitted resource answers the spelling: it names nothing, a declaration of
    /// another kind, or a declaration this project refused. The durable build reports
    /// all of those with the same row at the same span, so the distinction would be a
    /// retained cause no consumer reads — and the moment a steer to a refused
    /// declaration's own cause is wanted, minting it is a diagnostic change, not a
    /// binding change.
    Unbound,
}

impl<'a> StoreRow<'a> {
    pub(super) fn resolve(directory: &ResourceDirectory<'a>, store: &'a StoreDecl) -> Self {
        let resource = store.resource.as_str();
        let binding = match directory.lookup(resource) {
            Some(id) => StoreResourceBinding::Accepted(id),
            None => StoreResourceBinding::Unbound,
        };
        Self {
            resource,
            binding,
            indexes: IndexTable::take(&store.indexes),
            keys: KeyTable::take(
                KeyOwner::Store {
                    root: &store.root.root,
                    span: store.root.span,
                },
                &store.root.keys,
            ),
        }
    }

    /// The Product this store is an occurrence of, for the census.
    pub(super) fn product_key(&self) -> ProductKey<'a> {
        match self.binding {
            StoreResourceBinding::Accepted(id) => ProductKey::Bound(id),
            StoreResourceBinding::Unbound => ProductKey::Unbound(self.resource),
        }
    }
}

/// The Product a store declaration counts as an occurrence of.
///
/// A bound store counts under the resolved declaration it binds; an unbound one counts
/// under its written spelling, because that is all an unbound store has. Keying the
/// bound case on the resolved declaration is what the census's own retained note asked
/// for: distinct spellings cannot name one declaration, so the partition is the
/// Product's rather than the source text's, and it stays correct if resources ever stop
/// resolving project-globally.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ProductKey<'stores> {
    Bound(ResourceDeclId),
    Unbound(&'stores str),
}
/// One `index` declaration of a store root, as the durable build reads it: the name
/// every admission diagnostic renders, the uniqueness the suffix law turns on, the
/// declaration span the count and width caps report at, and the range of argument rows
/// this index projects.
pub(super) struct IndexRow<'a> {
    pub(super) name: &'a str,
    pub(super) unique: bool,
    pub(super) span: SourceSpan,
    args: Range<usize>,
}

/// How far one projection argument reaches.
///
/// A managed index projects the root's own leaves, so the only distinction the
/// admission rules draw over an argument's path is whether it stays at the top level
/// or reaches through a member. The row states that as the closed fact it is, rather
/// than leaving a segment count for each consumer to compare against one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IndexArgReach {
    /// A single-segment path: one of the root's identity keys or top-level fields.
    TopLevel,
    /// A dotted path of more than one segment, which no index may project.
    ThroughMember,
}

/// One projection argument of a managed index: the path spelling every diagnostic
/// about it renders, its own span, and how far it reaches.
///
/// The spelling is rendered once, here, from the parsed path segments. Every consumer
/// downstream compares and reports that one rendering instead of re-rendering the
/// path, which is what makes "the same component" one answer rather than one per
/// caller.
pub(super) struct IndexArgRow {
    pub(super) spelling: String,
    pub(super) span: SourceSpan,
    pub(super) reach: IndexArgReach,
}

/// One store root's managed indexes and their projection arguments, taken once from
/// the declaration before the root is built.
///
/// The two tables are one owner because an index without its arguments admits nothing:
/// every rule reads the index row and its argument rows together, and a range into a
/// single argument vector keeps that pairing a property of the table rather than of
/// each caller's bookkeeping.
pub(super) struct IndexTable<'a> {
    indexes: Vec<IndexRow<'a>>,
    args: Vec<IndexArgRow>,
}

impl<'a> IndexTable<'a> {
    pub(super) fn take(indexes: &'a [IndexDecl]) -> Self {
        let mut rows = Vec::with_capacity(indexes.len());
        let mut args = Vec::new();
        for index in indexes {
            let start = args.len();
            for arg in &index.args {
                args.push(IndexArgRow {
                    spelling: marrow_syntax::field_path_spelling(&arg.segments),
                    span: arg.span,
                    reach: if arg.segments.len() > 1 {
                        IndexArgReach::ThroughMember
                    } else {
                        IndexArgReach::TopLevel
                    },
                });
            }
            rows.push(IndexRow {
                name: &index.name,
                unique: index.unique,
                span: index.span,
                args: start..args.len(),
            });
        }
        Self {
            indexes: rows,
            args,
        }
    }

    pub(super) fn rows(&self) -> &[IndexRow<'a>] {
        &self.indexes
    }

    /// Each index row paired with the argument rows it projects, in declaration order.
    ///
    /// The pairing is the table's, not the caller's: an index row's argument range
    /// addresses this table's argument vector and nothing else, so handing out the two
    /// together is what keeps a row from ever being read against the wrong arguments.
    pub(super) fn entries(&self) -> impl Iterator<Item = (&IndexRow<'a>, &[IndexArgRow])> {
        self.indexes
            .iter()
            .map(|row| (row, &self.args[row.args.clone()]))
    }
}
/// Which declaration a durable key tuple belongs to: the anchor its columns hang
/// under, the span its width cap reports at, and the subject that cap names.
///
/// A root's key tuple and a branch's key tuple are the same shape enforced by the
/// same rules and anchored the same way, declared in two different places. Carrying
/// the difference as a closed owner is what lets the rules and the anchor join exist
/// once: the two sites used to spell the join `format!("{path}.{name}")` twice, in
/// two functions, and a divergence between them re-anchors committed durable identity
/// with no diagnostic anywhere.
pub(super) enum KeyOwner<'a> {
    /// A `store` root's key tuple, anchored at the root placement name.
    Store { root: &'a str, span: SourceSpan },
    /// A keyed `branch` placement's key tuple, anchored at the branch's member path.
    Member { path: &'a str, span: SourceSpan },
}

impl<'a> KeyOwner<'a> {
    /// The path every column of this tuple anchors under.
    fn anchor(&self) -> &'a str {
        match self {
            Self::Store { root, .. } => root,
            Self::Member { path, .. } => path,
        }
    }

    /// What the width-cap refusal calls this tuple.
    fn subject(&self) -> &'static str {
        match self {
            Self::Store { .. } => "a store root key tuple",
            Self::Member { .. } => "a branch key tuple",
        }
    }

    fn span(&self) -> SourceSpan {
        match self {
            Self::Store { span, .. } | Self::Member { span, .. } => *span,
        }
    }
}

/// One column of a durable key tuple: the name its ledger anchor ends with, and the
/// declared type its scalar is resolved from.
///
/// The column's position in [`KeyTable::rows`] is its declaration order, which is the
/// order the identity suffix law and the image key tuple both read.
pub(super) struct KeyRow<'a> {
    pub(super) name: &'a str,
    pub(super) ty: &'a TypeExpr,
}

/// One declaration's durable key tuple, taken once from the declaration.
pub(super) struct KeyTable<'a> {
    owner: KeyOwner<'a>,
    rows: Vec<KeyRow<'a>>,
}

impl<'a> KeyTable<'a> {
    pub(super) fn take(owner: KeyOwner<'a>, keys: &'a [KeyParam]) -> Self {
        let rows = keys
            .iter()
            .map(|key| KeyRow {
                name: &key.name,
                ty: &key.ty,
            })
            .collect();
        Self { owner, rows }
    }

    pub(super) fn rows(&self) -> &[KeyRow<'a>] {
        &self.rows
    }

    /// The span a refusal about this tuple as a whole reports at.
    pub(super) fn span(&self) -> SourceSpan {
        self.owner.span()
    }

    /// The width-cap refusal this tuple earns, or `None` when it fits.
    ///
    /// The cap is the image's fixed key-tuple width and applies to a root and a branch
    /// alike, so it is enforced here rather than once per declaring site.
    pub(super) fn over_wide(&self) -> Option<String> {
        (self.rows.len() > bounds::MAX_KEY_COLUMNS).then(|| {
            format!(
                "{} has {} columns; the fixed limit is {}",
                self.owner.subject(),
                self.rows.len(),
                bounds::MAX_KEY_COLUMNS
            )
        })
    }

    /// The ledger anchor path of one column: the owner's anchor, then the column name.
    ///
    /// This is the only place a key column's anchor is assembled. The anchors it
    /// returns are the keys of the machine-written `.marrow/ids` ledger, so a second
    /// spelling of this join would silently re-anchor durable identity — which is why
    /// the join has one owner and `durable_identity_stability.rs` freezes its output.
    pub(super) fn identity_path(&self, row: &KeyRow<'a>) -> String {
        format!("{}.{}", self.owner.anchor(), row.name)
    }
}
