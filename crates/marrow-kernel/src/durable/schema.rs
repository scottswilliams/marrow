//! The store's opaque bounded schema projection and its sole minter.
//!
//! A store schema describes one durable root: its key columns, its top-level fields, its
//! unkeyed groups, its keyed branches nested to any admitted depth, and its managed
//! indexes. The shape is recursive, which is exactly why none of these types can be built
//! by a caller.
//!
//! Every type here is opaque over private members and exposes only borrowed views. The one
//! route to a [`StoreSchema`] is [`StoreSchemaBuilder`], a flat stream of open/close
//! commands over an explicit stack that refuses a member opened past [`MAX_DURABLE_DEPTH`].
//! The bound is enforced at construction rather than at the entry points that consume a
//! schema, because entry-point validation cannot close this class: a caller holding a
//! recursive constructor overflows the stack while building its argument, and a refused
//! argument overflows again while it is dropped. A builder the caller drives with flat
//! commands has neither failure: a hostile stream costs `O(bound)` and returns a typed
//! refusal.

use crate::codec::value::{ScalarKind, ValueShape};

/// Nesting depth of the durable member tree the kernel serves, mirroring
/// `marrow_image::bounds::MAX_DURABLE_DEPTH`: a root's own member is depth 1, and a member
/// of a group or branch is one deeper. The kernel keeps no production dependency on the
/// image — it must bound its own construction with no image in hand — so the bound is
/// spelled here and held equal to the image's by the drift gate in
/// `tests/image_bound_drift.rs`.
pub const MAX_DURABLE_DEPTH: usize = 16;

/// The schema descriptor the store profile records and every session revalidates. One root;
/// its top-level fields, its unkeyed groups, its keyed branches in declaration (image)
/// order, and its managed indexes.
///
/// Opaque: built only by [`StoreSchemaBuilder`], read only through borrowed views. Its
/// branch tree is bounded by [`MAX_DURABLE_DEPTH`] at construction, so both the kernel's
/// own traversals and the implicit recursive `Drop` are bounded by the type itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSchema {
    root_name: String,
    key: Vec<ScalarKind>,
    fields: Vec<FieldSchema>,
    groups: Vec<GroupSchema>,
    branches: Vec<BranchSchema>,
    indexes: Vec<IndexSchema>,
}

impl StoreSchema {
    /// The root's declared name, retained for diagnostics and the store profile. No cell key
    /// is ever built from it.
    pub fn root_name(&self) -> &str {
        &self.root_name
    }

    /// The root's ordered key columns, the whole composite key. A single-column root is the
    /// one-element case.
    pub fn key(&self) -> &[ScalarKind] {
        &self.key
    }

    /// The root's top-level fields, in declaration order.
    pub fn fields(&self) -> &[FieldSchema] {
        &self.fields
    }

    /// The root's unkeyed groups, in declaration order.
    pub fn groups(&self) -> &[GroupSchema] {
        &self.groups
    }

    /// The root's keyed branches, in declaration order.
    pub fn branches(&self) -> &[BranchSchema] {
        &self.branches
    }

    /// The root's compiler-maintained managed indexes, in stable declaration order — the
    /// order every maintenance pass visits them, so a whole-entry write's index writes are
    /// deterministic.
    pub fn indexes(&self) -> &[IndexSchema] {
        &self.indexes
    }

    /// Resolve a top-level field by position, or `None` when the root declares no such
    /// field.
    pub(crate) fn field_pos(&self, field: u16) -> Option<FieldPos> {
        FieldPos::resolve(&self.fields, field)
    }

    /// Resolve an unkeyed group by position, or `None` when the root declares no such group.
    pub(crate) fn group_pos(&self, group: u16) -> Option<GroupPos> {
        GroupPos::resolve(&self.groups, group)
    }

    /// Resolve a managed index by position, or `None` when the root declares no such index.
    pub(crate) fn index_pos(&self, index: u16) -> Option<IndexPos> {
        IndexPos::resolve(&self.indexes, index)
    }
}

/// One field of a node's record: its name, value shape, and required flag. A field's shape
/// is the closed storable durable value set — a scalar, a dense product (`struct`/record),
/// or a closed sum (`enum`/`Option`/`Result`). The value codec (`codec::value`) frames a
/// composite within the one field-leaf cell; a scalar field stays byte-identical to the
/// pre-widening form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    name: String,
    shape: ValueShape,
    required: bool,
}

impl FieldSchema {
    /// The field's source name, retained for diagnostics only.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The field's storable value shape.
    pub fn shape(&self) -> &ValueShape {
        &self.shape
    }

    /// Whether a committed entry must carry this field.
    pub fn required(&self) -> bool {
        self.required
    }
}

/// One unkeyed group nested beneath a node: its name and its own record's fields. A group is
/// part of the entry's materialized value (a nested sub-record), not a keyed durable node:
/// it carries no marker and no key, and its presence is exactly its containing entry's
/// presence. Its leaves are stored as the entry's own payload, namespaced under the group's
/// number. A group holding nested groups or branches is not yet part of the executable
/// graph, so this schema carries fields only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSchema {
    name: String,
    fields: Vec<FieldSchema>,
}

impl GroupSchema {
    /// The group's declared name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The group's own record fields, in declaration order.
    pub fn fields(&self) -> &[FieldSchema] {
        &self.fields
    }
}

/// One keyed branch nested beneath a parent entry: its name, its ordered key columns, its
/// own record's fields, and its own nested branches. A branch entry is addressed by
/// extending the parent's key-path with the branch key and carries its own marker and field
/// leaves, so it is a distinct durable node reusing the parent entry's marker/field topology
/// one level down. The shape is recursive — a branch may itself declare keyed branches — so
/// the store profile describes a whole nested branch shape and a sub-branch shape change is
/// a profile mismatch at session open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSchema {
    name: String,
    key: Vec<ScalarKind>,
    fields: Vec<FieldSchema>,
    branches: Vec<BranchSchema>,
}

impl BranchSchema {
    /// The branch's declared name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The branch's ordered key columns. A branch entry's key-path extends its parent's with
    /// this whole tuple.
    pub fn key(&self) -> &[ScalarKind] {
        &self.key
    }

    /// The branch's own record fields, in declaration order.
    pub fn fields(&self) -> &[FieldSchema] {
        &self.fields
    }

    /// The branch's own nested branches, in declaration order.
    pub fn branches(&self) -> &[BranchSchema] {
        &self.branches
    }

    /// Resolve one of the branch's own fields by position, or `None` when it declares no
    /// such field.
    pub(crate) fn field_pos(&self, field: u16) -> Option<FieldPos> {
        FieldPos::resolve(&self.fields, field)
    }
}

/// A position resolved against a completed schema. Each of these is minted only by resolving
/// a caller-named `u16` against the table that declares it, so the lookup it later performs
/// is total: [`of`](FieldPos::of) reads any row *parallel to that table* — the schema's own
/// fields, the numbering that mirrors them, or the resolved record built from both — without
/// a second bounds question. This is why the site table resolves positions once, at
/// publication, rather than leaving raw indices for the session-setup resolver to trust.
macro_rules! schema_position {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) struct $name(u16);

        impl $name {
            /// Resolve a caller-named position against the table that declares it.
            pub(crate) fn resolve<T>(table: &[T], position: u16) -> Option<Self> {
                (usize::from(position) < table.len()).then_some(Self(position))
            }

            /// Read this position from any row parallel to the table it was resolved
            /// against.
            pub(crate) fn of<T>(self, row: &[T]) -> &T {
                &row[usize::from(self.0)]
            }
        }
    };
}

schema_position!(
    /// One durable root's position in a store projection's root table.
    RootPos
);
impl RootPos {
    /// The root's declaration position as a plain number: the resolved site carries it into
    /// the authorized site, which the physical layer keys the root's cell family by.
    pub(crate) fn index(self) -> u16 {
        self.0
    }
}

schema_position!(
    /// One field's position in the record that declares it.
    FieldPos
);
schema_position!(
    /// One unkeyed group's position in the root that declares it.
    GroupPos
);
schema_position!(
    /// One managed index's position in the root that declares it.
    IndexPos
);
schema_position!(
    /// One branch's position in the declaration-ordered branch list of its own level.
    BranchPos
);

/// One managed index the kernel maintains over a keyed root: its stable durable identity
/// (the physical cell discriminator that separates one index's cells from another's under
/// the same root, and survives a rename), its `unique` flag, and its ordered projection
/// resolved to record/key positions. The kernel stores the identity as raw bytes and stays
/// free of any image dependency. An index stores no data of its own and has no application
/// write path — maintenance is a consequence of the source write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSchema {
    id: [u8; 16],
    unique: bool,
    projection: Vec<IndexComponent>,
}

impl IndexSchema {
    /// The index's stable 16-byte identity, the physical discriminator of its cell family.
    pub fn id(&self) -> &[u8; 16] {
        &self.id
    }

    /// Whether a complete-key lookup yields at most one source key (a unique index) rather
    /// than an ordered non-unique index whose rows carry the identity suffix.
    pub fn unique(&self) -> bool {
        self.unique
    }

    /// The ordered projection, every component resolved against the completed root.
    pub fn projection(&self) -> &[IndexComponent] {
        &self.projection
    }
}

/// One component of a managed index's ordered projection, naming a durable-key leaf of the
/// root by position: an identity key column or a top-level field.
///
/// Opaque over its kind so a position can only enter a schema through
/// [`StoreSchemaBuilder::index`], which resolves it against the completed root's key columns
/// and field set. A positional component that names nothing therefore never reaches the
/// resolver, where it would have been an unchecked index into the root's tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexComponent(IndexComponentKind);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexComponentKind {
    Key(u16),
    Field(u16),
}

/// A borrowed view of one index projection component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexComponentRef {
    /// An identity key column, by its index into the root's key tuple.
    Key(u16),
    /// A top-level field, by its index into the root's materialized record.
    Field(u16),
}

impl IndexComponent {
    /// A component naming the root's key column `column`. The position is resolved against
    /// the completed root when the index is added, not here.
    pub fn key(column: u16) -> Self {
        Self(IndexComponentKind::Key(column))
    }

    /// A component naming the root's top-level field `field`. The position is resolved
    /// against the completed root when the index is added, not here.
    pub fn field(field: u16) -> Self {
        Self(IndexComponentKind::Field(field))
    }

    /// This component's kind and position.
    pub(crate) fn view(self) -> IndexComponentRef {
        match self.0 {
            IndexComponentKind::Key(column) => IndexComponentRef::Key(column),
            IndexComponentKind::Field(field) => IndexComponentRef::Field(field),
        }
    }
}

/// Why a flat schema command stream did not yield a store schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaBuildError {
    /// A member was placed at a nesting depth past [`MAX_DURABLE_DEPTH`]. Refused at the
    /// command, so the builder's own partial tree never grows past the bound either.
    TooDeep,
    /// A command was issued in a position the schema grammar has no place for: a group
    /// opened inside a group, a close with nothing open, or an index added while a group or
    /// branch was still open.
    Misplaced,
    /// The stream left a group or branch open.
    Unclosed,
    /// An index projection component named a key column or field position the completed root
    /// does not declare.
    UnresolvedIndexComponent,
    /// An index projection named a top-level field whose value shape is not a scalar, so it
    /// has no order-preserving key projection.
    NonScalarIndexComponent,
}

impl std::fmt::Display for SchemaBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooDeep => write!(f, "a durable member nests past its depth cap"),
            Self::Misplaced => write!(f, "a schema command has no place in the grammar"),
            Self::Unclosed => write!(f, "a schema stream left a group or branch open"),
            Self::UnresolvedIndexComponent => {
                write!(f, "an index projection names an undeclared position")
            }
            Self::NonScalarIndexComponent => {
                write!(f, "an index projection names a non-scalar field")
            }
        }
    }
}

impl std::error::Error for SchemaBuildError {}

/// One open member container of a schema under construction.
#[derive(Debug)]
enum SchemaFrame {
    Group {
        name: String,
        fields: Vec<FieldSchema>,
    },
    Branch {
        name: String,
        key: Vec<ScalarKind>,
        fields: Vec<FieldSchema>,
        branches: Vec<BranchSchema>,
    },
}

/// The sole minter of a [`StoreSchema`]: a flat stream of open/member/close commands over an
/// explicit stack, never a recursive value the caller assembles.
///
/// Commands latch the first refusal rather than returning per call, so a projection can emit
/// its whole stream and read one verdict at [`finish`](Self::finish). Nothing partially built
/// escapes: `finish` consumes the builder and yields a schema only when the stream was whole
/// and within [`MAX_DURABLE_DEPTH`].
#[derive(Debug)]
pub struct StoreSchemaBuilder {
    root_name: String,
    key: Vec<ScalarKind>,
    fields: Vec<FieldSchema>,
    groups: Vec<GroupSchema>,
    branches: Vec<BranchSchema>,
    indexes: Vec<IndexSchema>,
    stack: Vec<SchemaFrame>,
    /// Containers refused for depth, still awaiting their matching close so the stream's
    /// open/close balance is read correctly rather than reported as a second, spurious fault.
    suppressed: usize,
    error: Option<SchemaBuildError>,
}

impl StoreSchemaBuilder {
    /// Begin one root's schema: its name and its ordered key columns.
    pub fn root(name: impl Into<String>, key: Vec<ScalarKind>) -> Self {
        Self {
            root_name: name.into(),
            key,
            fields: Vec::new(),
            groups: Vec::new(),
            branches: Vec::new(),
            indexes: Vec::new(),
            stack: Vec::new(),
            suppressed: 0,
            error: None,
        }
    }

    /// Add a field to the innermost open container, or to the root when none is open.
    pub fn field(
        &mut self,
        name: impl Into<String>,
        shape: ValueShape,
        required: bool,
    ) -> &mut Self {
        if self.suppressed > 0 {
            return self;
        }
        if self.member_depth() > MAX_DURABLE_DEPTH {
            return self.fail(SchemaBuildError::TooDeep);
        }
        let field = FieldSchema {
            name: name.into(),
            shape,
            required,
        };
        match self.stack.last_mut() {
            Some(SchemaFrame::Group { fields, .. } | SchemaFrame::Branch { fields, .. }) => {
                fields.push(field);
            }
            None => self.fields.push(field),
        }
        self
    }

    /// Add a scalar-shaped field. Bounds the churn of the value-shape widening: a call site
    /// that knows a field is scalar names its kind rather than driving a shape builder.
    pub fn scalar_field(
        &mut self,
        name: impl Into<String>,
        kind: ScalarKind,
        required: bool,
    ) -> &mut Self {
        self.field(name, ValueShape::scalar(kind), required)
    }

    /// Open an unkeyed group. Its fields are the ones added until the matching
    /// [`close_group`](Self::close_group). A group nested inside another group or a branch is
    /// not part of the executable graph.
    pub fn open_group(&mut self, name: impl Into<String>) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed += 1;
            return self;
        }
        if !self.stack.is_empty() {
            return self.fail(SchemaBuildError::Misplaced);
        }
        if self.member_depth() > MAX_DURABLE_DEPTH {
            self.suppressed = 1;
            return self.fail(SchemaBuildError::TooDeep);
        }
        self.stack.push(SchemaFrame::Group {
            name: name.into(),
            fields: Vec::new(),
        });
        self
    }

    /// Close the open group.
    pub fn close_group(&mut self) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed -= 1;
            return self;
        }
        match self.stack.pop() {
            Some(SchemaFrame::Group { name, fields }) => {
                self.groups.push(GroupSchema { name, fields });
                self
            }
            Some(frame) => {
                self.stack.push(frame);
                self.fail(SchemaBuildError::Misplaced)
            }
            None => self.fail(SchemaBuildError::Misplaced),
        }
    }

    /// Open a keyed branch beneath the innermost open branch, or beneath the root when none
    /// is open. Its fields and sub-branches are the ones added until the matching
    /// [`close_branch`](Self::close_branch).
    pub fn open_branch(&mut self, name: impl Into<String>, key: Vec<ScalarKind>) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed += 1;
            return self;
        }
        if matches!(self.stack.last(), Some(SchemaFrame::Group { .. })) {
            return self.fail(SchemaBuildError::Misplaced);
        }
        if self.member_depth() > MAX_DURABLE_DEPTH {
            self.suppressed = 1;
            return self.fail(SchemaBuildError::TooDeep);
        }
        self.stack.push(SchemaFrame::Branch {
            name: name.into(),
            key,
            fields: Vec::new(),
            branches: Vec::new(),
        });
        self
    }

    /// Close the innermost open branch.
    pub fn close_branch(&mut self) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed -= 1;
            return self;
        }
        match self.stack.pop() {
            Some(SchemaFrame::Branch {
                name,
                key,
                fields,
                branches,
            }) => {
                let branch = BranchSchema {
                    name,
                    key,
                    fields,
                    branches,
                };
                match self.stack.last_mut() {
                    Some(SchemaFrame::Branch { branches, .. }) => branches.push(branch),
                    Some(SchemaFrame::Group { .. }) => {
                        unreachable!("a branch is never opened inside a group")
                    }
                    None => self.branches.push(branch),
                }
                self
            }
            Some(frame) => {
                self.stack.push(frame);
                self.fail(SchemaBuildError::Misplaced)
            }
            None => self.fail(SchemaBuildError::Misplaced),
        }
    }

    /// Add a managed index over the root, resolving every projection component against the
    /// root's completed key columns and top-level fields. An index is root-level, so it is
    /// refused while a group or branch is open.
    pub fn index(
        &mut self,
        id: [u8; 16],
        unique: bool,
        projection: Vec<IndexComponent>,
    ) -> &mut Self {
        if self.suppressed > 0 {
            return self;
        }
        if !self.stack.is_empty() {
            return self.fail(SchemaBuildError::Misplaced);
        }
        for component in &projection {
            match component.view() {
                IndexComponentRef::Key(column) => {
                    if usize::from(column) >= self.key.len() {
                        return self.fail(SchemaBuildError::UnresolvedIndexComponent);
                    }
                }
                IndexComponentRef::Field(position) => {
                    let Some(field) = self.fields.get(usize::from(position)) else {
                        return self.fail(SchemaBuildError::UnresolvedIndexComponent);
                    };
                    if field.shape.scalar_kind().is_none() {
                        return self.fail(SchemaBuildError::NonScalarIndexComponent);
                    }
                }
            }
        }
        self.indexes.push(IndexSchema {
            id,
            unique,
            projection,
        });
        self
    }

    /// The refusal this stream has already latched, if any. A projection driving a long
    /// stream reads it to stop early rather than emitting commands into a builder that has
    /// already decided.
    pub fn refusal(&self) -> Option<SchemaBuildError> {
        self.error
    }

    /// Consume the stream and yield the schema it described, or the first refusal it hit.
    pub fn finish(self) -> Result<StoreSchema, SchemaBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.stack.is_empty() || self.suppressed > 0 {
            return Err(SchemaBuildError::Unclosed);
        }
        Ok(StoreSchema {
            root_name: self.root_name,
            key: self.key,
            fields: self.fields,
            groups: self.groups,
            branches: self.branches,
            indexes: self.indexes,
        })
    }

    /// The nesting depth a member placed at the current position would carry: a root's own
    /// member is depth 1, and each open group or branch is one deeper.
    fn member_depth(&self) -> usize {
        self.stack.len() + 1
    }

    /// Latch the first refusal. Later commands are accepted and discarded so a projection
    /// need not branch mid-stream; `finish` reports this one verdict.
    fn fail(&mut self, error: SchemaBuildError) -> &mut Self {
        self.error.get_or_insert(error);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound is a construction-time refusal, not an entry-time one: a hostile stream of
    /// branch opens far past the depth cap returns a typed verdict, having retained only the
    /// bound's worth of frames. Before this row the same shape was a public recursive struct
    /// literal, and building it aborted the process.
    #[test]
    fn a_hostile_branch_stream_is_refused_at_the_bound() {
        let mut builder = StoreSchemaBuilder::root("root", vec![ScalarKind::Int]);
        for _ in 0..50_000 {
            builder.open_branch("deep", vec![ScalarKind::Int]);
        }
        for _ in 0..50_000 {
            builder.close_branch();
        }
        assert_eq!(builder.finish(), Err(SchemaBuildError::TooDeep));
    }

    /// N/N+1 on the member-depth bound. A branch opened at depth `d` places its own fields at
    /// depth `d + 1`, so the deepest admitted branch nesting is one short of the bound.
    #[test]
    fn the_member_depth_bound_admits_n_and_refuses_n_plus_one() {
        let nest = |levels: usize| {
            let mut builder = StoreSchemaBuilder::root("root", vec![ScalarKind::Int]);
            for _ in 0..levels {
                builder.open_branch("b", vec![ScalarKind::Int]);
            }
            builder.scalar_field("leaf", ScalarKind::Int, false);
            for _ in 0..levels {
                builder.close_branch();
            }
            builder.finish()
        };
        assert!(nest(MAX_DURABLE_DEPTH - 1).is_ok());
        assert_eq!(nest(MAX_DURABLE_DEPTH), Err(SchemaBuildError::TooDeep));
    }

    /// An index projection is resolved against the completed root, so a position naming
    /// nothing never reaches the resolver's tables.
    #[test]
    fn an_index_projection_resolves_against_the_completed_root() {
        let build = |projection: Vec<IndexComponent>| {
            let mut builder = StoreSchemaBuilder::root("root", vec![ScalarKind::Int]);
            builder.scalar_field("title", ScalarKind::Str, true);
            builder.index([0; 16], true, projection);
            builder.finish()
        };
        assert!(build(vec![IndexComponent::field(0), IndexComponent::key(0)]).is_ok());
        assert_eq!(
            build(vec![IndexComponent::field(1)]),
            Err(SchemaBuildError::UnresolvedIndexComponent),
        );
        assert_eq!(
            build(vec![IndexComponent::key(1)]),
            Err(SchemaBuildError::UnresolvedIndexComponent),
        );
    }

    /// An unclosed container yields no schema: nothing partially built escapes the builder.
    #[test]
    fn an_unclosed_container_yields_no_schema() {
        let mut builder = StoreSchemaBuilder::root("root", vec![ScalarKind::Int]);
        builder.open_branch("b", vec![ScalarKind::Int]);
        assert_eq!(builder.finish(), Err(SchemaBuildError::Unclosed));
    }
}
