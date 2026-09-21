//! The declaration-side row tables the durable build reads instead of syntax.
//!
//! Each table is taken once, before the first store is built, from the owners its own
//! subject has: the declaration the parser wrote, and, where a type must resolve, the
//! fact the type registry admitted for it — `IndexTable` reads syntax alone. Member
//! paths, key widths, key columns, and index arguments are settled here, so a consumer
//! is handed validated projections rather than a question it could answer a second way.
//!
//! Declaration syntax is still reachable past these tables: the build receives the raw
//! `resource` declarations, each `StoreDecl` travels beside its row, and a [`GroupRow`]
//! retains its member `FieldDecl`s. Each row states what it retains.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::source::ProjectFile;
use marrow_image::bounds;
use marrow_syntax::{
    FieldDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, StoreDecl,
};

use crate::analysis::FileRef;
use crate::decl::MemberNamespace;
use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;
use crate::types::{GenericInvariant, RecordInfo, ResolveError, ResolveRefusal, TypeRegistry};
/// A typed handle to one admitted `resource` declaration: a row index into the
/// [`ResourceDirectory`] that minted it, never on the wire.
///
/// The handle carries no brand for its directory, so a second directory would accept it;
/// the build takes one directory per compile, so there is no second one to reach.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResourceDeclId(usize);

/// One `resource` declaration as the durable builder reads it: the record the type
/// registry admitted, and the member `group`/`branch` tree projected from the
/// declaration the parser wrote.
///
/// The row holds no `ResourceDecl` of its own: the declaration is borrowed for the
/// projection and left where it was.
pub(super) struct ResourceRow<'a> {
    pub(super) file: &'a ProjectFile,
    pub(super) record: &'a RecordInfo,
    pub(super) groups: Vec<GroupRow<'a>>,
}

/// Every `resource` declaration the type registry admitted, addressed by
/// [`ResourceDeclId`] and looked up by written spelling.
///
/// This is the one join between a resource's two owners — the declaration the parser
/// wrote and the record the type registry admitted — performed once, before any store
/// is built. The registry drives it: every row comes from an admitted record, so which
/// resources exist is decided by exactly one owner, and a store reaches a record only
/// *through* a row this join built. An admitted record whose cited declaration is
/// absent, or sits somewhere other than the declare pass recorded, is a compiler
/// coherence failure raised here and nowhere else.
///
/// The join key is the ordinal the declare pass recorded, not the written name.
pub(super) struct ResourceDirectory<'a> {
    rows: Vec<ResourceRow<'a>>,
    by_spelling: BTreeMap<&'a str, ResourceDeclId>,
}

impl<'a> ResourceDirectory<'a> {
    pub(super) fn take(
        resources: &'a [(FileRef, ProjectFile, &'a ResourceDecl)],
        records: &'a TypeRegistry,
    ) -> Result<Self, GenericInvariant> {
        // The declare pass paired every admitted record with its declaration by pushing
        // both in lockstep; this reads that pairing rather than rebuilding it from name
        // spellings, which are not declaration identity. The coordinate check requires
        // the declaration at each cited ordinal to sit at the module position and name
        // span the declare pass recorded, so a declaration that moved is refused rather
        // than paired with whatever now sits at that index. It does not authenticate the
        // slice: `FileRef` is snapshot-local and `FileIdentity` is not compared.
        let ordinals = records.record_declaration_ordinals();
        let admitted = records.admitted_resources();
        let mut rows = Vec::with_capacity(admitted.len());
        let mut by_spelling = BTreeMap::new();
        for (index, record) in admitted.iter().enumerate() {
            let missing = || GenericInvariant::DurableResourceMissing(record.type_id);
            let (at, file, decl) = ordinals
                .get(index)
                .and_then(|&ordinal| resources.get(ordinal))
                .ok_or_else(missing)?;
            let (declared_at, declared_span) = records
                .declaration_module(record.type_id)
                .ok_or_else(missing)?;
            if declared_at != *at || declared_span != decl.name_span {
                return Err(missing());
            }
            let id = ResourceDeclId(rows.len());
            rows.push(ResourceRow {
                file,
                record,
                groups: group_rows(file, records, &record.name, &decl.members)?,
            });
            by_spelling.insert(record.name.as_str(), id);
        }
        Ok(Self { rows, by_spelling })
    }

    fn lookup(&self, spelling: &str) -> Option<ResourceDeclId> {
        self.by_spelling.get(spelling).copied()
    }

    /// The row `id` addresses. `id` is minted only by [`Self::take`], from a length taken
    /// immediately before the matching push, so it always addresses a live row.
    pub(super) fn row(&self, id: ResourceDeclId) -> &ResourceRow<'a> {
        &self.rows[id.0]
    }
}

/// One `store` declaration's resource binding, resolved before any store is built.
///
/// The written spelling is carried beside the binding because every diagnostic the
/// binding produces renders it; a row holding only the resolution would send its
/// consumer back to the declaration.
pub(super) struct StoreRow<'a> {
    pub(super) resource: &'a str,
    pub(super) binding: StoreResourceBinding,
    /// The root's managed indexes, taken from the declaration with the binding so the
    /// build reads no `index` syntax of its own.
    pub(super) indexes: IndexTable<'a>,
    /// The root's identity key tuple, taken and resolved with the same reading, so the
    /// build re-resolves nothing.
    pub(super) keys: KeyTable<'a>,
}

/// What a `store` declaration's written resource spelling binds to.
pub(super) enum StoreResourceBinding {
    /// The spelling names a `resource` declaration the type registry admitted.
    Accepted(ResourceDeclId),
    /// No admitted resource answers the spelling: it names nothing, a declaration of
    /// another kind, or one this project refused. The durable build reports all of those
    /// with the same row at the same span, so the cause is not retained.
    Unbound,
}

impl<'a> StoreRow<'a> {
    pub(super) fn resolve(
        directory: &ResourceDirectory<'a>,
        store: &'a StoreDecl,
        records: &TypeRegistry,
        file: &ProjectFile,
    ) -> Result<Self, GenericInvariant> {
        let resource = store.resource.as_str();
        let binding = match directory.lookup(resource) {
            Some(id) => StoreResourceBinding::Accepted(id),
            None => StoreResourceBinding::Unbound,
        };
        let keys = KeyTable::take(
            KeyOwner::Store {
                root: &store.root.root,
                span: store.root.span,
            },
            &store.root.keys,
            file,
            records,
        )?;
        Ok(Self {
            resource,
            binding,
            indexes: IndexTable::take(&store.indexes),
            keys,
        })
    }

    /// The census key this store counts under: the Product it binds, or its written spelling.
    pub(super) fn product_key(&self) -> ProductKey<'a> {
        match self.binding {
            StoreResourceBinding::Accepted(id) => ProductKey::Bound(id),
            StoreResourceBinding::Unbound => ProductKey::Unbound(self.resource),
        }
    }
}

/// What a store declaration counts as an occurrence under: a Product, or an unbound spelling.
///
/// A bound store counts under the resolved declaration it binds; an unbound one counts
/// under its written spelling, because that is all an unbound store has. Keying the
/// bound case on the declaration rather than the text partitions by Product — distinct
/// spellings cannot name one declaration — and stays correct if resources ever stop
/// resolving project-globally.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ProductKey<'stores> {
    Bound(ResourceDeclId),
    Unbound(&'stores str),
}
/// One `index` declaration of a store root, as the durable build reads it: the name
/// the per-index diagnostics render, the uniqueness the suffix law turns on, the
/// declaration span the count and width caps report at, and the range of argument rows
/// this index projects.
pub(super) struct IndexRow<'a> {
    pub(super) name: &'a str,
    pub(super) name_span: SourceSpan,
    pub(super) unique: bool,
    pub(super) span: SourceSpan,
    args: Range<usize>,
}

/// How far one projection argument reaches.
///
/// A managed index projects the root's own leaves, so an argument's path shape decides
/// only whether it stays at the top level or reaches through a member; which leaf a
/// top-level name reaches is resolved later. Stating the shape as a closed fact keeps
/// each consumer from re-comparing a segment count.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IndexArgReach {
    /// A single-segment path: resolution later admits it as a key or a field, or rejects it.
    TopLevel,
    /// A dotted path of more than one segment, which no index may project.
    ThroughMember,
}

/// One projection argument of a managed index: the path spelling every diagnostic
/// about it renders, its own span, and how far it reaches.
///
/// The spelling is rendered once, here, from the parsed path segments; every consumer
/// compares and reports that one rendering, so "the same component" has one answer
/// rather than one per caller.
pub(super) struct IndexArgRow {
    pub(super) spelling: String,
    pub(super) span: SourceSpan,
    pub(super) reach: IndexArgReach,
}

/// One store root's managed indexes and their projection arguments, taken once from
/// the declaration before the root is built.
///
/// The two tables are one owner because an index's admission ends in its arguments: the
/// width cap and component resolution read the index row and its argument rows together.
/// A range into a single argument vector keeps that pairing a property of the table.
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
                name_span: index.name_span,
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
    /// An index row's argument range addresses this table's argument vector and nothing
    /// else, so handing the two out together keeps a row from being read against the
    /// wrong arguments.
    pub(super) fn entries(&self) -> impl Iterator<Item = (&IndexRow<'a>, &[IndexArgRow])> {
        self.indexes
            .iter()
            .map(|row| (row, &self.args[row.args.clone()]))
    }
}
/// Which declaration a durable key tuple belongs to: the anchor its columns hang
/// under, the span its width cap reports at, and the subject that cap names.
///
/// A root's key tuple and a branch's key tuple are the same shape enforced by the same
/// rules and anchored the same way, declared in two different places. Carrying the
/// difference as a closed owner lets the rules and the anchor join exist once: two
/// spellings of that join re-anchor durable identity, reported as a `.marrow/ids` gap
/// on the new anchor and never as a rename of the old one.
pub(super) enum KeyOwner<'a> {
    /// A `store` root's key tuple, anchored at the root placement name.
    Store { root: &'a str, span: SourceSpan },
    /// A keyed `branch` placement's key tuple, anchored at the branch's member path.
    /// The path is owned: it is assembled once, when the branch's row is taken.
    Member { path: String, span: SourceSpan },
}

impl KeyOwner<'_> {
    /// The path every column of this tuple anchors under.
    fn anchor(&self) -> &str {
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

/// One validated column of a durable key tuple: the name its ledger anchor ends with
/// and the scalar its declared type resolved to.
///
/// The declared type is not kept. A tuple is admitted whole or refused whole, so a row
/// that exists has already passed the closed durable-key scalar set and no consumer
/// holds the annotation a second resolution would need.
struct KeyColumnRow<'a> {
    spelling: &'a str,
    scalar: ScalarType,
}

/// One declaration's durable key tuple: its declared width, and its columns resolved
/// to the durable-key scalar set or the one refusal that resolution earned.
///
/// A root's tuple and a branch's tuple are the same shape enforced by the same rules,
/// so they are one owner, taken once per declared tuple per compile. The table retains
/// none of the `KeyParam`s it was taken from — the width is a count, a column is a name
/// and a scalar, the refusal is a settled diagnostic — so a consumer cannot resolve a
/// column's scalar a second time off it. Column position is declaration order
/// throughout, the order the identity suffix law and the image key tuple both read.
pub(super) struct KeyTable<'a> {
    owner: KeyOwner<'a>,
    /// The declared column count. Kept beside the resolution because the width cap
    /// names how many columns were *written*, which a refused tuple has none of.
    declared_width: usize,
    resolution: Result<Vec<KeyColumnRow<'a>>, Box<SourceDiagnostic>>,
}

/// One admitted key column as a consumer reads it: the name the image key entry
/// carries, the ledger anchor it resolves under, and its scalar.
pub(super) struct AdmittedKeyColumn<'a> {
    pub(super) spelling: &'a str,
    pub(super) anchor: String,
    pub(super) scalar: ScalarType,
}

/// What a key tuple admits, in the order its refusals are ranked.
///
/// The three arms are the whole answer, so a consumer cannot read the columns without
/// having been handed the refusal that would have made them wrong. Both readings rank
/// through [`KeyTable::admitted`], so a tuple that is both over-wide and unresolvable is
/// refused for its width at each.
pub(super) enum KeyColumns<'a> {
    Admitted(Vec<AdmittedKeyColumn<'a>>),
    /// The tuple is over the image's fixed key width: the refusal and its span.
    OverWide {
        span: SourceSpan,
        message: String,
    },
    /// A column's declared type is outside the durable-key scalar set; the row that
    /// says so, settled when this tuple was taken.
    Unresolved(&'a SourceDiagnostic),
}

impl<'a> KeyTable<'a> {
    /// Take and resolve one declared tuple.
    ///
    /// The fields are private, so this is the only constructor outside this module.
    /// Resolution happens here rather than at a consumer: it is a fact of the tuple,
    /// settled once.
    pub(super) fn take(
        owner: KeyOwner<'a>,
        keys: &'a [KeyParam],
        file: &ProjectFile,
        records: &TypeRegistry,
    ) -> Result<Self, GenericInvariant> {
        let resolution = resolve_key_columns(file, &owner, keys, records)?;
        Ok(Self {
            owner,
            declared_width: keys.len(),
            resolution,
        })
    }

    /// This tuple's admission verdict, and on admission every column a consumer reads.
    pub(super) fn columns(&self) -> KeyColumns<'_> {
        match self.admitted() {
            Ok(columns) => KeyColumns::Admitted(
                columns
                    .iter()
                    .map(|column| AdmittedKeyColumn {
                        spelling: column.spelling,
                        anchor: self.identity_path(column.spelling),
                        scalar: column.scalar,
                    })
                    .collect(),
            ),
            Err(refusal) => refusal,
        }
    }

    /// The columns this tuple admits, or the refusal that outranks them.
    ///
    /// The one place the width cap and the scalar resolution are ranked against each
    /// other, so neither can lead at one reading and trail at another. The width cap is
    /// a fact of the declared tuple: a tuple past it has no admitted columns to report
    /// whatever its columns resolved to.
    fn admitted(&self) -> Result<&[KeyColumnRow<'a>], KeyColumns<'_>> {
        if let Some(message) = self.over_wide() {
            return Err(KeyColumns::OverWide {
                span: self.owner.span(),
                message,
            });
        }
        match &self.resolution {
            Ok(columns) => Ok(columns),
            Err(row) => Err(KeyColumns::Unresolved(row)),
        }
    }

    /// The settled scalar tuple, or the typed coherence failure for a consumer that can
    /// only run once this tuple was admitted. A refused member refuses its store, so a
    /// store reaching the executable derivation has proved every tuple both resolved and
    /// within the width cap; either refusal therefore answers with a coherence failure.
    pub(super) fn resolved(&self) -> Result<Vec<ScalarType>, GenericInvariant> {
        match self.admitted() {
            Ok(columns) => Ok(columns.iter().map(|column| column.scalar).collect()),
            Err(_) => Err(GenericInvariant::DurableBranchKeyUnresolved),
        }
    }

    /// The width-cap refusal this tuple earns, or `None` when it fits.
    ///
    /// The cap is the image's fixed key-tuple width and applies to a root and a branch
    /// alike, so it is enforced here rather than once per declaring site.
    fn over_wide(&self) -> Option<String> {
        (self.declared_width > bounds::MAX_KEY_COLUMNS).then(|| {
            format!(
                "{} has {} columns; the fixed limit is {}",
                self.owner.subject(),
                self.declared_width,
                bounds::MAX_KEY_COLUMNS
            )
        })
    }

    /// The ledger anchor path of one column: the owner's anchor, then the column name.
    ///
    /// The only place a key column's anchor is assembled. These anchors are the keys of
    /// the machine-written `.marrow/ids` ledger, so a second spelling of this join
    /// re-anchors durable identity: the compiler reports the new anchor as a
    /// missing-identity gap and the mint action commits it beside the id the old
    /// spelling still owns.
    fn identity_path(&self, spelling: &str) -> String {
        format!("{}.{}", self.owner.anchor(), spelling)
    }
}
/// One `group` member of a resource — a static namespace or, when keyed, a `branch`
/// placement — as the durable build reads it: the declaration it was taken from, its
/// qualified path, its key rows when keyed, and its nested group rows in declaration
/// order.
///
/// The tree mirrors the declaration's group nesting exactly, so a walker drives off the
/// rows and re-derives neither a member path nor keyedness from syntax. Taken once per
/// compile with the directory: a store attempt that stages and rolls back consumes the
/// same rows a later attempt does.
pub(super) struct GroupRow<'a> {
    /// The member's simple name — what the physical layer keys a branch family by,
    /// and the segment its path ends with.
    pub(super) name: &'a str,
    /// The qualified member path, the branch-path and key-anchor prefix: the one
    /// assembly of it from declaration syntax. `DurableRegistry` joins the same shape
    /// again over built descriptors, in `record_branch_declarations` and as the lookup
    /// key of `declares_branch`.
    pub(super) path: String,
    /// The member's directly declared stored fields, in declaration order.
    ///
    /// The `group` declaration itself is not retained, so its own key tuple reaches a
    /// consumer only as [`GroupRow::keys`]. A `FieldDecl` does carry its own `KeyParam`s,
    /// which `DurableRegistry::build_field` reads to refuse a keyed field, so a consumer
    /// holding this row holds that much key syntax.
    pub(super) fields: Vec<&'a FieldDecl>,
    /// The span of the first declared member, for the depth-cap refusal.
    pub(super) first_member_span: Option<SourceSpan>,
    /// `Some` for a keyed `branch` placement, `None` for a static `group`.
    pub(super) keys: Option<KeyTable<'a>>,
    pub(super) groups: Vec<GroupRow<'a>>,
}

/// Project the group rows of `members`, in declaration order, recursively.
fn group_rows<'a>(
    file: &ProjectFile,
    records: &TypeRegistry,
    container: &str,
    members: &'a [ResourceMember],
) -> Result<Vec<GroupRow<'a>>, GenericInvariant> {
    let mut rows = Vec::new();
    for member in members {
        let ResourceMember::Group(group) = member else {
            continue;
        };
        let path = format!("{container}.{}", group.name);
        let keys = (!group.keys.is_empty())
            .then(|| {
                KeyTable::take(
                    KeyOwner::Member {
                        path: path.clone(),
                        span: group.span,
                    },
                    &group.keys,
                    file,
                    records,
                )
            })
            .transpose()?;
        let groups = group_rows(file, records, &path, &group.members)?;
        rows.push(GroupRow {
            name: &group.name,
            fields: group
                .members
                .iter()
                .filter_map(|member| match member {
                    ResourceMember::Field(field) => Some(field),
                    _ => None,
                })
                .collect(),
            first_member_span: group
                .members
                .first()
                .map(marrow_syntax::ResourceMember::span),
            path,
            keys,
            groups,
        });
    }
    Ok(rows)
}

/// Resolve each declared key column in tuple order, rejecting a key type outside the
/// closed orderable durable-key set. A singleton placement has no columns and yields
/// an empty vector. Called only from [`KeyTable::take`], so it is the sole reader of a
/// key column's declared type and the resolution is a fact of the table.
///
/// The rejection row is returned rather than pushed, so the build refuses a store from
/// the row that reports it. It is boxed because a diagnostic is wide next to a key
/// column vector and this is the refused arm, never the admitted column loop.
fn resolve_key_columns<'a>(
    file: &ProjectFile,
    owner: &KeyOwner<'a>,
    keys: &'a [KeyParam],
    records: &TypeRegistry,
) -> Result<Result<Vec<KeyColumnRow<'a>>, Box<SourceDiagnostic>>, GenericInvariant> {
    let span = owner.span();
    // A root's tuple is the store's own layer, claimed here. A branch's tuple is one
    // layer with the members it keys, claimed by the branch's declaration in the
    // type registry, once whatever number of stores bind the resource.
    let mut root_layer = match owner {
        KeyOwner::Store { root, .. } => Some(MemberNamespace::new(format!("^{root}"))),
        KeyOwner::Member { .. } => None,
    };
    let mut columns = Vec::with_capacity(keys.len());
    for column in keys {
        if let Some(row) = root_layer
            .as_mut()
            .and_then(|layer| layer.claim(file, &column.name, column.name_span))
        {
            return Ok(Err(Box::new(row)));
        }
        let key = match records.scalar_annotation(file.origin(), &column.ty) {
            Ok(key) => key,
            Err(ResolveError::Refusal(refusal)) => {
                let span = match refusal {
                    ResolveRefusal::RefusedDeclaration(_) => column.ty.span(),
                    _ => span,
                };
                let row = records.scalar_refusal_row(refusal, file, span, "this key type")?;
                return Ok(Err(Box::new(row)));
            }
            Err(ResolveError::Invariant(invariant)) => return Err(invariant),
        };
        if !super::orderable_durable_key(key) {
            return Ok(Err(Box::new(SourceDiagnostic::at(
                marrow_codes::Code::CheckType,
                file,
                span,
                "a durable key column must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                    .to_string(),
            ))));
        }
        columns.push(KeyColumnRow {
            spelling: column.name.as_str(),
            scalar: key,
        });
    }
    Ok(Ok(columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tuple past the width cap is refused at every reading of its table, the settled
    /// scalars included.
    ///
    /// Production asks for the verdict before the scalars, so no source can exercise
    /// this; the state is built directly because the property belongs to the type rather
    /// than to a caller ordering.
    #[test]
    fn an_over_wide_tuple_is_refused_at_the_scalar_reading_too() {
        let store = || KeyOwner::Store {
            root: "root",
            span: SourceSpan::default(),
        };
        let column = || KeyColumnRow {
            spelling: "k",
            scalar: ScalarType::Int,
        };
        let over_wide = KeyTable {
            owner: store(),
            declared_width: bounds::MAX_KEY_COLUMNS + 1,
            resolution: Ok(vec![column()]),
        };
        assert!(matches!(over_wide.columns(), KeyColumns::OverWide { .. }));
        assert!(
            over_wide.resolved().is_err(),
            "a tuple the width cap refuses has no settled scalar tuple to hand out",
        );
        let within = KeyTable {
            owner: store(),
            declared_width: 1,
            resolution: Ok(vec![column()]),
        };
        assert_eq!(
            within.resolved().expect("a tuple within the cap resolves"),
            vec![ScalarType::Int],
        );
    }
}
