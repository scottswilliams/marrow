//! The declaration-side row tables the durable build reads instead of syntax.
//!
//! Each table is taken once, before the first store is built, from the two owners a
//! durable declaration has: the declaration the parser wrote, and the fact the type
//! registry admitted for it. The build then reaches a declaration only through a row,
//! so a consumer cannot re-derive an answer this module already decided, and the two
//! owners disagreeing about one name is unrepresentable rather than reported.

use std::collections::BTreeMap;
use std::ops::Range;

use marrow_project::FileIdentity;
use marrow_syntax::{IndexDecl, ResourceDecl, SourceSpan, StoreDecl};

use crate::analysis::FileRef;
use crate::decl::{Binding, DeclarationIndexDrift};
use crate::types::{RecordInfo, TypeRegistry};
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
/// any store is built. Downstream a store reaches a record only *through* a row that
/// already holds the declaration it was built from, so the two owners disagreeing
/// about one name is unrepresentable rather than reported: that is what retires the
/// invariant a second, declaration-side lookup by name used to raise.
pub(super) struct ResourceDirectory<'a> {
    rows: Vec<ResourceRow<'a>>,
    by_spelling: BTreeMap<&'a str, ResourceDeclId>,
}

impl<'a> ResourceDirectory<'a> {
    pub(super) fn take(
        resources: &[(FileRef, FileIdentity, &'a ResourceDecl)],
        records: &'a TypeRegistry,
    ) -> Self {
        let mut rows = Vec::new();
        let mut by_spelling = BTreeMap::new();
        for (_, _, decl) in resources {
            let Some(record) = records.by_name(&decl.name) else {
                continue;
            };
            let id = ResourceDeclId(rows.len());
            rows.push(ResourceRow { decl, record });
            // A repeated resource name is refused by the declare pass, which keeps the
            // first declaration; answering the spelling with the first row is the same
            // choice, spelled once.
            by_spelling.entry(decl.name.as_str()).or_insert(id);
        }
        Self { rows, by_spelling }
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
}

/// What a `store` declaration's written resource spelling binds to.
pub(super) enum StoreResourceBinding {
    /// The spelling names a `resource` declaration the type registry admitted.
    Accepted(ResourceDeclId),
    /// The spelling names a declaration this project refused, so the name is written
    /// but binds no admitted shape.
    ///
    /// The refusal's own handle is deliberately not carried. The durable build reports
    /// the refused and the absent case with the same row at the same span, so a handle
    /// here would be a retained cause no consumer can read — and the moment a steer to
    /// that cause is wanted, it is the `named_type` lookup below that mints it, one
    /// line from where this arm is decided.
    Refused,
    /// No admitted or refused declaration answers the spelling as a resource — it
    /// names nothing, or it names a declaration of another kind.
    Absent,
}

impl<'a> StoreRow<'a> {
    pub(super) fn resolve(
        directory: &ResourceDirectory<'a>,
        records: &TypeRegistry,
        store: &'a StoreDecl,
    ) -> Result<Self, DeclarationIndexDrift> {
        let resource = store.resource.as_str();
        let binding = match directory.lookup(resource) {
            Some(id) => StoreResourceBinding::Accepted(id),
            None => match records.named_type(resource)? {
                Binding::Refused(..) => StoreResourceBinding::Refused,
                Binding::Accepted(_) | Binding::Absent => StoreResourceBinding::Absent,
            },
        };
        Ok(Self {
            resource,
            binding,
            indexes: IndexTable::take(&store.indexes),
        })
    }

    /// The Product this store is an occurrence of, for the census.
    pub(super) fn product_key(&self) -> ProductKey<'a> {
        match self.binding {
            StoreResourceBinding::Accepted(id) => ProductKey::Bound(id),
            StoreResourceBinding::Refused | StoreResourceBinding::Absent => {
                ProductKey::Unbound(self.resource)
            }
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
