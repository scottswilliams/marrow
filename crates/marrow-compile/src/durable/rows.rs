//! The declaration-side row tables the durable build reads instead of syntax.
//!
//! Each table is taken once, before the first store is built, from the two owners a
//! durable declaration has: the declaration the parser wrote, and the fact the type
//! registry admitted for it. Member paths, key widths, key columns, and index
//! arguments are settled here, so a consumer is handed validated projections rather
//! than a question it could answer a second way.
//!
//! Declaration syntax stays reachable through these tables, and that is the state of
//! the build today rather than an oversight. `DurableRegistry::build` still receives
//! the raw `resource` declarations the tables are constructed from; each store's own
//! `StoreDecl` travels beside its row for the root placement name and the spans its
//! refusals report at; and a [`GroupRow`] retains its member `FieldDecl`s, which carry
//! their own key syntax. Each row states what it retains. Closing those routes means
//! the build accepting rows and nothing else, which is a successor row's work.

use std::collections::BTreeMap;
use std::ops::Range;

use marrow_image::bounds;
use marrow_project::FileIdentity;
use marrow_syntax::{
    FieldDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, StoreDecl,
};

use crate::analysis::FileRef;
use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;
use crate::types::{GenericInvariant, RecordInfo, TypeRegistry};
/// A typed handle to one admitted `resource` declaration, valid only in the
/// [`ResourceDirectory`] that minted it.
///
/// A row index rather than a narrowed ordinal: it addresses this compile's
/// declaration list and never reaches the wire.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResourceDeclId(usize);

/// One `resource` declaration as the durable builder reads it: the record the type
/// registry admitted, and the member `group`/`branch` tree projected from the
/// declaration the parser wrote.
///
/// The row holds no `ResourceDecl` of its own: the declaration is borrowed for the
/// projection and left where it was. It stays reachable to the build through the slice
/// `DurableRegistry::build` receives, so this row narrows what the build reads without
/// putting the declaration out of reach.
pub(super) struct ResourceRow<'a> {
    pub(super) record: &'a RecordInfo,
    pub(super) groups: Vec<GroupRow<'a>>,
}

/// Every `resource` declaration the type registry admitted, addressed by
/// [`ResourceDeclId`] and looked up by written spelling.
///
/// This is the one join between a resource's two owners — the declaration the parser
/// wrote and the record the type registry admitted — and it is performed once, before
/// any store is built. The registry drives the join: every row comes from an admitted
/// record, so which resources exist is decided by exactly one owner, and a store
/// reaches a record only *through* a row that already holds a declaration. An
/// admitted record no declaration in the received slice answers is the two build
/// inputs drifting apart — a compiler coherence failure raised here, at the single
/// join, and nowhere else.
///
/// The join is the ordinal the declare pass recorded, not the written name, and
/// [`Self::take`] states what that pairing does and does not establish.
pub(super) struct ResourceDirectory<'a> {
    rows: Vec<ResourceRow<'a>>,
    by_spelling: BTreeMap<&'a str, ResourceDeclId>,
}

impl<'a> ResourceDirectory<'a> {
    pub(super) fn take(
        resources: &[(FileRef, FileIdentity, &'a ResourceDecl)],
        records: &'a TypeRegistry,
    ) -> Result<Self, GenericInvariant> {
        // The declare pass already paired every admitted record with the declaration it was
        // built from, by pushing both in lockstep, so this reads that pairing rather than
        // rebuilding one. It used to rebuild it from resource name spellings, which was two
        // defects in one: source spelling is not declaration identity, so a same-named
        // declaration from elsewhere paired happily; and re-deriving a fact an earlier
        // owner settled is the re-derivation the speed pillar forbids. Reading the ordinal
        // is linear and admits no key at all.
        //
        // What the ordinal establishes: for the writer that recorded it — the declare pass,
        // handed the same slice — an exact record-to-declaration pairing, derived once and
        // never again. What the coordinate check below adds: the declaration at that
        // ordinal must sit at the module POSITION and name span the declare pass recorded,
        // so a slice of a different shape fails loudly instead of pairing by position with
        // whatever it was handed. A position, not a spelling, because two parses of one
        // project repeat spellings and cannot repeat positions.
        //
        // What neither establishes: that an arbitrary caller's slice is the one the
        // registry was built from. `FileRef` is snapshot-local and `FileIdentity` is not
        // compared, so a second parse presenting the same module ordinal and name span with
        // its members mutated is accepted here. Making that unrepresentable means carrying
        // the declare pass's pairing out with the registry under one borrowed wrapper,
        // which retires the ordinal and this check together; it is a successor row's work.
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
                record,
                groups: group_rows(file, records, &record.name, &decl.members),
            });
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
    /// The root's identity key tuple, taken and resolved with the same reading. The
    /// build renders a refusal at the position it always held; it re-resolves nothing.
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
    pub(super) fn resolve(
        directory: &ResourceDirectory<'a>,
        store: &'a StoreDecl,
        records: &TypeRegistry,
        file: &FileIdentity,
    ) -> Self {
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
        );
        Self {
            resource,
            binding,
            indexes: IndexTable::take(&store.indexes),
            keys,
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
/// that exists has already passed the closed durable-key scalar set, and no consumer
/// holds the annotation a second resolution would need.
struct KeyColumnRow<'a> {
    spelling: &'a str,
    scalar: ScalarType,
}

/// One declaration's durable key tuple: its declared width, and its columns resolved
/// to the durable-key scalar set or the one refusal that resolution earned.
///
/// A root's tuple and a branch's tuple are the same shape enforced by the same rules,
/// so they are one owner. It is taken once per declared tuple per compile, and this
/// table retains none of the `KeyParam`s it was taken from: the width is a count, a
/// column is a name and a scalar, and the refusal is a settled diagnostic, so a
/// consumer cannot resolve a column's scalar a second time off the table it was handed.
/// The claim is the table's alone — a keyed field's own key syntax travels with the
/// `FieldDecl` in [`GroupRow::fields`]. Column position is declaration order
/// throughout, which is the order the identity suffix law and the image key tuple both
/// read.
pub(super) struct KeyTable<'a> {
    owner: KeyOwner<'a>,
    /// The declared column count. Kept beside the resolution because the width cap
    /// names how many columns were *written*, which a refused tuple no longer has.
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
/// having been handed the refusal that would have made them wrong. Both readings of a
/// table rank through [`KeyTable::admitted`], so a tuple that is both over-wide and
/// unresolvable answers `OverWide` at either of them — the precedence both declaring
/// sites always had, carried by the type rather than by the order its callers ask in.
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
    /// Every key-table construction — root or branch, wherever it is spelled — charges
    /// the once-per-compile counter here, so a reconstruction cannot avoid the count by
    /// living at a different call site. Resolution happens here rather than at a
    /// consumer: it is a fact of the tuple, settled once.
    pub(super) fn take(
        owner: KeyOwner<'a>,
        keys: &'a [KeyParam],
        file: &FileIdentity,
        records: &TypeRegistry,
    ) -> Self {
        #[cfg(test)]
        crate::types::bump_key_table_construction();
        let resolution = resolve_key_columns(file, owner.span(), keys, records);
        Self {
            owner,
            declared_width: keys.len(),
            resolution,
        }
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
    /// other. Every reading of a table is answered from here, so neither can be read
    /// ahead of the other at one reading and behind it at another: the width cap is a
    /// fact of the declared tuple, and a tuple past it has no admitted columns to
    /// report whatever its columns resolved to.
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
    /// only run once this tuple was admitted. The graph build consumes a refusal as the
    /// declaring member's own diagnostic, and a refused member refuses its store, so a
    /// store that reached the executable derivation proved every tuple admitted — both
    /// resolved and within the width cap, which is why either refusal answers here with
    /// the same coherence failure rather than with a scalar tuple.
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
    /// This is the only place a key column's anchor is assembled. The anchors it
    /// returns are the keys of the machine-written `.marrow/ids` ledger, so a second
    /// spelling of this join would silently re-anchor durable identity — which is why
    /// the join has one owner and `durable_identity_stability.rs` freezes its output.
    /// It is private because a column is: [`Self::columns`] is the only way to reach an
    /// anchor, so no caller is in a position to spell the join a second time.
    fn identity_path(&self, spelling: &str) -> String {
        format!("{}.{}", self.owner.anchor(), spelling)
    }
}
/// One `group` member of a resource — a static namespace or, when keyed, a `branch`
/// placement — as the durable build reads it: the declaration it was taken from, the
/// qualified path every walker used to assemble on its own, its key rows when keyed,
/// and its nested group rows in declaration order.
///
/// The tree mirrors the declaration's group nesting exactly, so a walker drives off
/// the rows and can no longer re-derive a path or reclassify keyedness. It is taken
/// once per compile, with the directory: a store attempt that stages and rolls back
/// consumes the same rows a later attempt does.
pub(super) struct GroupRow<'a> {
    /// The member's simple name — what the physical layer keys a branch family by,
    /// and the segment its path ends with.
    pub(super) name: &'a str,
    /// The qualified member path, the branch-path and key-anchor prefix. Assembled
    /// here and nowhere else.
    pub(super) path: String,
    /// The member's directly declared stored fields, in declaration order.
    ///
    /// The `group` declaration itself is not retained, so its own key tuple reaches a
    /// consumer only as [`GroupRow::keys`]. A field's key tuple is another matter: a
    /// `FieldDecl` carries its `KeyParam`s, and `DurableRegistry::build_field` reads
    /// them to refuse a keyed field. A consumer holding this row therefore holds that
    /// much key syntax, and will until a field row displaces the declaration here.
    pub(super) fields: Vec<&'a FieldDecl>,
    /// The span of the first declared member, for the depth-cap refusal.
    pub(super) first_member_span: Option<SourceSpan>,
    /// `Some` for a keyed `branch` placement, `None` for a static `group`.
    pub(super) keys: Option<KeyTable<'a>>,
    pub(super) groups: Vec<GroupRow<'a>>,
}

/// Project the group rows of `members`, in declaration order, recursively.
fn group_rows<'a>(
    file: &FileIdentity,
    records: &TypeRegistry,
    container: &str,
    members: &'a [ResourceMember],
) -> Vec<GroupRow<'a>> {
    let mut rows = Vec::new();
    for member in members {
        let ResourceMember::Group(group) = member else {
            continue;
        };
        let path = format!("{container}.{}", group.name);
        let keys = (!group.keys.is_empty()).then(|| {
            KeyTable::take(
                KeyOwner::Member {
                    path: path.clone(),
                    span: group.span,
                },
                &group.keys,
                file,
                records,
            )
        });
        let groups = group_rows(file, records, &path, &group.members);
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
    rows
}
/// Resolve each declared key column in tuple order, rejecting a key type outside the
/// closed orderable durable-key set. A singleton placement has no columns and yields
/// an empty vector. Called only from [`KeyTable::take`], so it is the sole reader of a
/// key column's declared type and the resolution is a fact of the table.
///
/// The rejection row is returned rather than pushed: the build refuses a store with
/// it at the position the refusal always held, and a refusal is summarized from the
/// row that reports it in one statement. It is boxed because a diagnostic is wide
/// next to a key column vector and this path is the refused arm, never the admitted
/// column loop.
fn resolve_key_columns<'a>(
    file: &FileIdentity,
    span: SourceSpan,
    keys: &'a [KeyParam],
    records: &TypeRegistry,
) -> Result<Vec<KeyColumnRow<'a>>, Box<SourceDiagnostic>> {
    let mut columns = Vec::with_capacity(keys.len());
    for column in keys {
        let Some(key) = super::scalar_of(&records.expand(&column.ty)) else {
            return Err(Box::new(super::unsupported(file, span, "this key type")));
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
                marrow_codes::Code::CheckType.as_str(),
                file,
                span,
                "a durable key column must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                    .to_string(),
            )));
        }
        columns.push(KeyColumnRow {
            spelling: column.name.as_str(),
            scalar: key,
        });
    }
    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tuple past the width cap is refused at every reading of its table, the settled
    /// scalars included.
    ///
    /// Production asks for the verdict before the scalars, so no compile reaches
    /// `resolved` on an over-wide tuple and no source can exercise this; the state is
    /// built here directly because the property is the type's rather than a caller
    /// ordering's. Ranking the width cap at only one of the two readings restores the
    /// bypass and fails here.
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
