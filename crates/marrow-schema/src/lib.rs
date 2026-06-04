//! Compiles parsed Marrow resource and store declarations into schema shapes.
//!
//! [`ResourceSchema`] describes the typed resource tree: fields, keyed layers,
//! groups, and saved-field value rules. [`StoreSchema`] owns the durable root,
//! identity keys, and indexes that attach a resource shape to saved data. Semantic
//! validation beyond structure is deferred; see the notes on [`compile_resource`]
//! and [`compile_store`].

use std::fmt;

use marrow_syntax::{
    EnumDecl, EnumMember, FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember,
    SourceSpan, StoreDecl, TypeRef,
};

pub mod stdlib;

// The canonical scalar type lives in marrow-store; re-export it so resolution
// and downstream crates share one import path for the storable scalars.
pub use marrow_store::value::ScalarType;

/// A type annotation resolved once during schema compilation, so downstream
/// crates match on structure instead of re-parsing the source spelling.
///
/// Resolution is structural and module-blind: it decides everything a single
/// declaration can (a scalar, a `sequence[...]`, `Id(^store)`, or `unknown`), and
/// leaves any other bare or qualified name as [`Type::Named`]. The checker,
/// which knows the project's resource and enum names, promotes a `Named` to a
/// resource or enum reference or flags it unknown; the runtime uses the checked
/// resolver when it must backstop constructor field types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Scalar(ScalarType),
    Sequence(Box<Type>),
    /// A store identity such as `Id(^books)`, carrying the store root name.
    Identity(String),
    /// A bare or qualified name that is not a scalar, sequence, identity, or
    /// `unknown`: a resource reference (the checker confirms it) or a typo.
    Named(String),
    /// The explicit dynamic boundary type `unknown`.
    Unknown,
}

impl Type {
    /// Resolve a [`TypeRef`]'s source spelling to its structure. Total and
    /// module-blind: every spelling maps to exactly one [`Type`], with anything
    /// not decidable from the text alone landing in [`Type::Named`].
    pub fn resolve(ty: &TypeRef) -> Self {
        Self::resolve_text(ty.text.trim())
    }

    fn resolve_text(text: &str) -> Self {
        // `sequence[T]` is built-in element-type sugar; recurse on the element.
        if let Some(element) = sequence_element(text) {
            return Self::Sequence(Box::new(Self::resolve_text(element)));
        }
        if let Some(scalar) = ScalarType::from_scalar_name(text) {
            return Self::Scalar(scalar);
        }
        if text == "unknown" {
            return Self::Unknown;
        }
        if let Some(store) = text
            .strip_prefix("Id(^")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Self::Identity(store.to_string());
        }
        Self::Named(text.to_string())
    }

    /// The scalar this type denotes, or `None` for a sequence, identity, named,
    /// or unknown type. The runtime decodes a saved leaf by this scalar.
    pub fn scalar(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            _ => None,
        }
    }

    /// The scalar envelope for a plain saved leaf. Named types need project-level
    /// resolution before the compiler can attach their durable value meaning.
    pub fn stored_scalar(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            _ => None,
        }
    }

    /// Does this type embed `unknown`? A type embeds `unknown` when it is
    /// `unknown` itself or a `sequence[...]` whose element embeds it. Managed
    /// saved schemas reject `unknown` anywhere inside.
    pub fn embeds_unknown(&self) -> bool {
        match self {
            Self::Unknown => true,
            Self::Sequence(element) => element.embeds_unknown(),
            _ => false,
        }
    }
}

impl fmt::Display for Type {
    /// The canonical source spelling, the inverse of [`Type::resolve`]. Used in
    /// rejection messages that name the offending type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(scalar) => f.write_str(scalar.name()),
            Self::Sequence(element) => write!(f, "sequence[{element}]"),
            Self::Identity(store) => write!(f, "Id(^{store})"),
            Self::Named(name) => f.write_str(name),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// The compiled tree shape of a resource declaration.
///
/// Members are kept in source order in one `Vec` rather than a map: a resource
/// has few members, lookups are linear, and order matches the declaration and
/// inspect output. Fields and keyed layers interleave as declared; consumers
/// that want only one kind filter `members` by [`NodeKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub members: Vec<Node>,
}

impl ResourceSchema {
    /// The declared type of a stored field named by its saved-path chain — the
    /// named segments after the identity, outermost first, where the last name is
    /// a scalar field and every earlier name is a group layer to descend into. A
    /// single-name chain reads a top-level field; a longer chain descends the
    /// leading names as groups and reads the last name as a field of the innermost
    /// group. An empty chain, or any name the schema does not declare as that
    /// shape, resolves to `None`.
    ///
    /// A keyed-leaf layer read at the same position is [`Self::leaf_type`]; the two
    /// differ only in whether the terminal name is a field (a [`NodeKind::Slot`]) or
    /// a group (a [`NodeKind::Group`]) to descend, so both share the one walk.
    pub fn field_type(&self, chain: &[&str]) -> Option<&Type> {
        let (&leaf, groups) = chain.split_last()?;
        // No lead names is a top-level field; otherwise descend the lead names as
        // group layers and read the terminal as a field of the innermost group.
        let members = match groups {
            [] => &self.members,
            _ => &self.descend_layers(groups)?.members,
        };
        plain_field(members, leaf)
    }

    /// The declared leaf value type of a keyed-leaf layer named by its chain of
    /// layer names, outermost first. The last name is the keyed-leaf layer being
    /// read; earlier names are the groups to descend through. Resolves to `None`
    /// for an empty chain, an unknown layer, or a group layer (which has members,
    /// not a leaf value).
    pub fn leaf_type(&self, layers: &[&str]) -> Option<&Type> {
        match &self.descend_layers(layers)?.kind {
            NodeKind::Slot { ty, .. } => Some(ty),
            NodeKind::Group => None,
        }
    }

    /// Descend a non-empty chain of group layer names, following nested layers,
    /// and return the innermost layer node. `None` for an empty chain or an
    /// unknown name. Also used by the runtime to check that a layer chain is fully
    /// declared before touching the store. A plain field (a `Slot` with no key
    /// parameters) is not a layer, so a name resolving to one fails the descent.
    pub fn descend_layers(&self, layers: &[&str]) -> Option<&Node> {
        let (&first, rest) = layers.split_first()?;
        let mut current = layer_member(&self.members, first)?;
        for &name in rest {
            current = layer_member(&current.members, name)?;
        }
        Some(current)
    }
}

/// The value type of a *plain* field member named `name`: a `Slot` with no key
/// parameters. A keyed leaf or a group of the same name is not a plain field, so
/// it resolves to `None`.
fn plain_field<'a>(members: &'a [Node], name: &str) -> Option<&'a Type> {
    members.iter().find_map(|node| match &node.kind {
        NodeKind::Slot { ty, .. } if node.name == name && node.key_params.is_empty() => Some(ty),
        _ => None,
    })
}

/// The layer node named `name`: a group or a keyed leaf — anything but a plain
/// field (a `Slot` with no key parameters), which is not a layer to descend.
fn layer_member<'a>(members: &'a [Node], name: &str) -> Option<&'a Node> {
    members
        .iter()
        .find(|node| node.name == name && !node.is_plain_field())
}

impl Node {
    /// Whether this node is a plain top-level (or group-member) field: a `Slot`
    /// carrying no key parameters. A keyed leaf (`Slot` with key parameters) and a
    /// group are layers, not plain fields. The write planner and whole-resource
    /// read use this to pick out the fields they materialize.
    pub fn is_plain_field(&self) -> bool {
        self.key_params.is_empty() && matches!(self.kind, NodeKind::Slot { .. })
    }

    /// The type of this node when it is a plain field, else `None`. Lets a caller
    /// select plain fields and bind their type in one pass.
    pub fn plain_field_type(&self) -> Option<&Type> {
        match &self.kind {
            NodeKind::Slot { ty, .. } if self.key_params.is_empty() => Some(ty),
            _ => None,
        }
    }

    /// The type a single value cell of this node holds, for any [`NodeKind::Slot`]: a
    /// plain field's own type, or a keyed-leaf-layer (`map[K, V]`) entry's value type V.
    /// A group holds no single value cell and resolves to `None`. Evolution records this
    /// as the member's identity-aware leaf token, so a value-type change is detected by
    /// referent identity for a keyed-leaf value the same way it is for a plain field.
    pub fn leaf_value_type(&self) -> Option<&Type> {
        match &self.kind {
            NodeKind::Slot { ty, .. } => Some(ty),
            NodeKind::Group => None,
        }
    }
}

/// The durable root and identity key shape declared by a store. Identity keys
/// live in the saved path; they are not stored fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRootSchema {
    pub root: String,
    pub identity_keys: Vec<KeyDef>,
}

impl SavedRootSchema {
    /// Does this store root qualify for the default `nextId` allocation policy?
    /// Only a store with exactly one `int` identity key does; composite
    /// identities, non-integer identities, and keyless singletons are
    /// application-provided. This is the one contract both the checker (which
    /// types `nextId(^root)`) and the runtime write planner (which allocates the
    /// next id) gate on, so it lives here on the shape they both key off.
    pub fn single_int_root(&self) -> bool {
        matches!(self.identity_keys.as_slice(), [key] if key.ty == Type::Scalar(ScalarType::Int))
    }

    /// Name the identity shape that disqualifies this root from the default
    /// `nextId` policy, as a noun phrase for the rejection message: a keyless
    /// singleton, a single non-`int` key, or a composite identity. Both the
    /// checker diagnostic and the runtime fault reuse this so their wording
    /// cannot drift apart.
    pub fn next_id_shape(&self) -> String {
        match self.identity_keys.as_slice() {
            [] => "a keyless singleton".into(),
            [key] => format!("a single `{}` key", key.ty),
            keys => format!("a composite identity of {} keys", keys.len()),
        }
    }
}

/// The compiled durable store over a resource tree shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSchema {
    pub root: String,
    pub resource: String,
    pub docs: Vec<String>,
    pub identity_keys: Vec<KeyDef>,
    pub indexes: Vec<IndexSchema>,
}

impl StoreSchema {
    pub fn identity_type(&self) -> Type {
        Type::Identity(self.root.clone())
    }

    pub fn saved_root(&self) -> SavedRootSchema {
        SavedRootSchema {
            root: self.root.clone(),
            identity_keys: self.identity_keys.clone(),
        }
    }

    pub fn single_int_root(&self) -> bool {
        self.saved_root().single_int_root()
    }

    pub fn next_id_shape(&self) -> String {
        self.saved_root().next_id_shape()
    }
}

/// A named, typed key parameter of a saved root or keyed layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDef {
    pub name: String,
    pub ty: Type,
}

/// One node of the resource tree: a top-level field, a keyed leaf, or a group,
/// distinguished by its [`NodeKind`]. The recursive `members` are filled only for
/// a group; a keyed leaf carries `key_params` and an empty `members`; a
/// top-level field carries neither key params nor members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub docs: Vec<String>,
    /// Empty for a top-level field; non-empty for any keyed leaf or keyed group.
    pub key_params: Vec<KeyDef>,
    /// Empty for any [`NodeKind::Slot`]; the nested nodes for an [`NodeKind::Group`].
    pub members: Vec<Node>,
    pub kind: NodeKind,
}

/// What a [`Node`] holds: a scalar value (`Slot`) or nested members (`Group`).
///
/// A top-level field and a keyed-leaf layer are both `Slot`s — the keyed leaf is
/// a `Slot` with non-empty `key_params`. A group (`notes(noteId: string)` /
/// `versions(version)` / an unkeyed `name`) is a `Group` with nested `members`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// A scalar value: a top-level field or a keyed leaf. `required` varies only
    /// on a top-level/group field; a keyed leaf never exposes it (always false).
    Slot { ty: Type, required: bool },
    /// A keyed or unkeyed group, whose value lives in the node's `members`.
    Group,
}

/// A declared lookup index over identity keys and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub args: Vec<String>,
    pub unique: bool,
}

/// The compiled form of an enum: a named, fixed set of members. Members are held
/// flat in pre-order DFS; those positions are source traversal indices for tree
/// queries, not durable value identity. The tree shape lives in each member's
/// `parent` link. An enum is its own construct, not a [`ResourceSchema`]; it owns
/// no saved root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumSchema {
    pub name: String,
    pub docs: Vec<String>,
    /// Members in pre-order DFS; a member's index is a source traversal handle.
    pub members: Vec<EnumMemberSchema>,
}

/// One enum member. `parent` is the traversal index of the enclosing member,
/// `None` at the top level. A `category` member groups its descendants and is not
/// selectable as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMemberSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub parent: Option<usize>,
    pub category: bool,
}

/// The outcome of walking a relative member path against an [`EnumSchema`]. The
/// one walk behind value, `is`, and `match` arm resolution returns this so each
/// caller applies its own position rule (selectability) to a single resolved
/// member and reports ambiguity with the same actionable wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberPathResolution {
    /// The path names exactly this member traversal index.
    Found(usize),
    /// A single bare name appears under more than one parent, so it cannot pick
    /// one member. Carries the full disambiguating paths of every match
    /// (`["tiger::paw", "lion::paw"]`), in pre-order, for the diagnostic.
    Ambiguous(Vec<String>),
    /// No member the path could walk to. Either the first segment is not a member
    /// of the enum, or a later segment is not a child of the member before it.
    NotFound,
}

impl EnumSchema {
    /// The source traversal index of `member`, or `None` if the enum has no such
    /// member. When two members at different levels share a bare name, the first
    /// in pre-order wins; the checker rejects an ambiguous reference before this
    /// is reached for a value or arm.
    pub fn ordinal(&self, member: &str) -> Option<usize> {
        self.members.iter().position(|m| m.name == member)
    }

    /// Walk a relative member path (`["tiger", "bengal"]`) to a single member. A
    /// qualified path starts at a top-level member and walks parent→child, one
    /// segment per level; since names are unique among siblings the walk is always
    /// unambiguous. A bare single name may sit at any depth and is the one position
    /// a duplicate can leave unresolved: the same name under different parents
    /// (`tiger::paw`, `lion::paw`) is [`MemberPathResolution::Ambiguous`].
    ///
    /// The single shared walk behind value, `is`, and `match` arm resolution: each
    /// caller decides whether the resolved member is valid for its position (a value
    /// rejects a category; an `is` operand or an arm admits one). The ambiguity case
    /// carries the qualifying paths so every caller reports the same fix.
    pub fn walk_member_path(&self, path: &[&str]) -> MemberPathResolution {
        let Some((&first, rest)) = path.split_first() else {
            return MemberPathResolution::NotFound;
        };
        // A bare single name searches the whole tree by name: exactly one match
        // resolves, several (the same name under different parents) is ambiguous.
        if rest.is_empty() {
            let matches: Vec<usize> = (0..self.members.len())
                .filter(|&ordinal| self.member_is(ordinal, first))
                .collect();
            return match matches.as_slice() {
                [] => MemberPathResolution::NotFound,
                [ordinal] => MemberPathResolution::Found(*ordinal),
                _ => MemberPathResolution::Ambiguous(
                    matches
                        .iter()
                        .map(|&ordinal| self.member_path(ordinal).join("::"))
                        .collect(),
                ),
            };
        }
        // A qualified path starts at the top level (`tiger` in `tiger::paw`) and
        // walks children downward. Top-level names are themselves unique only per
        // sibling level, but a qualified path's leading segment must be a top-level
        // member; if several share the name the path cannot pick one.
        let roots: Vec<usize> = (0..self.members.len())
            .filter(|&ordinal| {
                self.members[ordinal].parent.is_none() && self.member_is(ordinal, first)
            })
            .collect();
        let [start] = roots.as_slice() else {
            return MemberPathResolution::NotFound;
        };
        let mut current = *start;
        for &segment in rest {
            match self.child_named(current, segment) {
                Some(child) => current = child,
                None => return MemberPathResolution::NotFound,
            }
        }
        MemberPathResolution::Found(current)
    }

    /// Whether the member at this traversal index has the given name.
    fn member_is(&self, ordinal: usize, name: &str) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.name == name)
    }

    /// The traversal index of the child of `parent` named `name`, if any.
    /// Children are the members whose `parent` link is `parent`; sibling names are
    /// unique, so at most one matches.
    fn child_named(&self, parent: usize, name: &str) -> Option<usize> {
        (0..self.members.len()).find(|&ordinal| {
            self.members[ordinal].parent == Some(parent) && self.members[ordinal].name == name
        })
    }

    /// The member name a traversal index selects, or `None` if it is out of range.
    pub fn member_name(&self, ordinal: usize) -> Option<&str> {
        self.members.get(ordinal).map(|m| m.name.as_str())
    }

    /// Whether one traversal index sits at or under `ancestor` in the member tree
    /// — the `is` primitive. Inclusive: a member is its own descendant, so a
    /// concrete leaf on both sides is exact equality and a category ancestor
    /// matches its whole subtree.
    pub fn is_descendant(&self, ordinal: usize, ancestor: usize) -> bool {
        let mut current = Some(ordinal);
        while let Some(index) = current {
            if index == ancestor {
                return true;
            }
            current = self.members.get(index).and_then(|member| member.parent);
        }
        false
    }

    /// Whether the member at this traversal index is a category — a grouping node
    /// that is not selectable as a value but may name a whole subtree in `match`
    /// or `is`.
    pub fn is_category(&self, ordinal: usize) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.category)
    }

    /// The traversal indices at or under `ancestor`, inclusive — the members a
    /// category arm or an `is` test covers.
    pub fn subtree_ordinals(&self, ancestor: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_descendant(ordinal, ancestor))
    }

    /// The traversal indices for concrete leaves. A category is never selectable,
    /// and a member with children is a grouping node whose value is one of its
    /// descendants, so only childless non-category members are selectable.
    pub fn selectable_leaves(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_selectable_leaf(ordinal))
    }

    /// Whether a traversal index is a selectable leaf: concrete (not a category)
    /// and with no children.
    pub fn is_selectable_leaf(&self, ordinal: usize) -> bool {
        let Some(member) = self.members.get(ordinal) else {
            return false;
        };
        !member.category && !self.has_children(ordinal)
    }

    /// Whether any member names this traversal index as its parent.
    pub fn has_children(&self, ordinal: usize) -> bool {
        self.members.iter().any(|m| m.parent == Some(ordinal))
    }

    /// The dotted path of names from the root to `ordinal` (`["tiger", "bengal"]`),
    /// for diagnostics. Empty when the traversal index is out of range.
    pub fn member_path(&self, ordinal: usize) -> Vec<&str> {
        let mut path = Vec::new();
        let mut current = Some(ordinal);
        while let Some(index) = current {
            let Some(member) = self.members.get(index) else {
                break;
            };
            path.push(member.name.as_str());
            current = member.parent;
        }
        path.reverse();
        path
    }
}

/// An error found while compiling a resource into a schema.
///
/// `code` is a stable `schema.*` identifier; `message` is human-readable; and
/// `span` points at the offending declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
}

/// A resource member name collides with another member at the same level.
pub const SCHEMA_DUPLICATE_MEMBER: &str = "schema.duplicate_member";

/// A `category` enum member has no nested members. A category groups its
/// descendants, so one with nothing under it can never be selected as a value nor
/// matched, leaving it dead.
pub const SCHEMA_CATEGORY_LEAF: &str = "schema.category_leaf";

/// A non-`category` enum member has nested members. A member with children is a
/// grouping node: a value selects one of its descendants, never the node itself,
/// and a `match` covers its leaves, never the node. Marking such a parent
/// `category` is what keeps the two value-validity notions aligned — value position
/// rejects exactly the categories, while `match` covers exactly the childless
/// non-categories — so a parent left unmarked would be a legal value no arm could
/// cover. The invariant category <=> has-children makes that fail-open impossible.
pub const SCHEMA_PARENT_NOT_CATEGORY: &str = "schema.parent_not_category";

/// An index appears inside a group. Indexes are direct members of keyed saved
/// resources; nested-layer lookups are modeled as a separate resource.
pub const SCHEMA_INDEX_IN_GROUP: &str = "schema.index_in_group";

/// A managed saved field or key is typed `unknown`. `unknown` is a dynamic
/// boundary value; saved schemas use concrete field and key types. Local-only
/// resources may use `unknown`.
pub const SCHEMA_UNKNOWN_IN_SAVED: &str = "schema.unknown_in_saved";

/// A parsed type spelling is only supported in a narrower declaration context.
/// Currently `map[K, V]` is declaration sugar for saved keyed-leaf members
/// only; it is not a general local or nested map type.
pub const SCHEMA_UNSUPPORTED_TYPE: &str = "schema.unsupported_type";

/// A top-level field or layer shares a name with an identity key. Identity keys
/// live in the saved path, so a stored member of the same name is ambiguous.
pub const SCHEMA_KEY_MEMBER_COLLISION: &str = "schema.key_member_collision";

/// An index argument does not resolve to an identity key or a top-level field.
/// Index arguments do not walk keyed child layers or unkeyed group descendants.
pub const SCHEMA_UNKNOWN_INDEX_ARG: &str = "schema.unknown_index_arg";

/// A saved key (an identity key, a keyed-layer key parameter, or an index
/// argument) has a type with no order-preserving key encoding — currently
/// `decimal`. Saved keys use ordered key types; the store cannot encode a
/// decimal as a key, so the write planner could never maintain such an entry.
/// Reject it at compile time rather than commit data with an unmaintained index
/// or key.
pub const SCHEMA_UNORDERABLE_KEY: &str = "schema.unorderable_key";

/// A non-unique index does not end with all identity keys in declaration order.
/// A non-unique entry is a presence marker, so two records sharing the indexed
/// values would collapse onto one entry unless the identity keys make each entry
/// distinct. A unique index is exempt: each populated entry already points to one
/// identity.
pub const SCHEMA_INDEX_MISSING_IDENTITY_KEYS: &str = "schema.index_missing_identity_keys";

/// An index is declared on a store with no keyed saved root. Declared indexes
/// need a store identity for entries to point to.
pub const SCHEMA_INDEX_REQUIRES_KEYED_ROOT: &str = "schema.index_requires_keyed_root";

/// An index argument names a field nested through an unkeyed group. The write
/// planner matches index arguments by flat top-level name, so it would silently
/// never maintain such an entry. Until nested index resolution lands, reject it.
pub const SCHEMA_NESTED_INDEX_ARG: &str = "schema.nested_index_arg";

/// A managed saved field's type is a bare name that is not a declared enum. A
/// saved field stores a scalar or a checked enum value; an undefined name or a
/// resource type has no saved leaf form, so it cannot be a saved field.
pub const SCHEMA_NON_ENUM_NAMED_FIELD: &str = "schema.non_enum_named_field";

/// A saved key (an identity key, a keyed-layer key parameter, or an index
/// argument) is typed as a non-scalar. A key must be an orderable scalar, because
/// the store projects a key from its scalar value. Every bare or qualified name
/// in identity-key and keyed-layer positions — a local enum, a cross-module enum,
/// a resource, or a typo — every sequence, and every store identity is rejected
/// structurally, since the rule asks only whether the type is an orderable scalar.
pub const SCHEMA_NONSCALAR_KEY: &str = "schema.nonscalar_key";

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.span.line, self.span.column, self.code, self.message
        )
    }
}

impl std::error::Error for SchemaError {}

/// Compile a parsed resource declaration into a [`ResourceSchema`].
///
/// Always returns a best-effort schema together with any errors, so callers can
/// keep checking. Maps the resource tree shape and the single-resource rules the
/// schema alone can decide, including saved-field value types and keyed-layer
/// key types. Store identity keys and index arguments are checked by
/// [`compile_store`].
///
/// Deferred: full type validation and one-owner-per-root.
pub fn compile_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    compile_resource_shape(decl, false)
}

/// Compile a resource shape that is attached to at least one store declaration.
pub fn compile_stored_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    compile_resource_shape(decl, true)
}

fn compile_resource_shape(
    decl: &ResourceDecl,
    saved_map_sugar: bool,
) -> (ResourceSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();

    let mut members = Vec::new();
    let mut names = Namespace::default();

    for member in &decl.members {
        names.check(member_name(member), member_span(member), &mut errors);
        members.push(member_node(member, &mut errors, saved_map_sugar));
    }

    let schema = ResourceSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        members,
    };

    check_unsupported_map_types(&decl.members, saved_map_sugar, &mut errors);
    (schema, errors)
}

/// Compile a parsed store declaration into a [`StoreSchema`] against the resource
/// shape it stores.
pub fn compile_store(
    decl: &StoreDecl,
    resource: &ResourceSchema,
) -> (StoreSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();
    check_duplicate_key_params(&decl.root.keys, decl.span, &mut errors);
    for key in &decl.root.keys {
        if let Some(error) = unsupported_map_key_param_error(key, decl.span) {
            errors.push(error);
            continue;
        }
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("identity key", &key.name, decl.span));
        } else if let Some(error) = key_type_error("identity key", &key.name, &ty, decl.span) {
            errors.push(error);
        }
        if resource
            .members
            .iter()
            .any(|member| member.name == key.name)
        {
            errors.push(SchemaError {
                code: SCHEMA_KEY_MEMBER_COLLISION,
                message: format!(
                    "identity key `{}` collides with a top-level member of the \
                     same name; identity keys live in the saved path, not stored \
                     members",
                    key.name
                ),
                span: decl.span,
            });
        }
    }

    let mut names = Namespace::default();
    for index in &decl.indexes {
        names.check(&index.name, index.span, &mut errors);
        if decl.root.keys.is_empty() {
            errors.push(SchemaError {
                code: SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
                message: format!(
                    "index `{}` requires a keyed saved root; a singleton store has \
                     no identity for an index entry to point to",
                    index.name
                ),
                span: index.span,
            });
            continue;
        }
        if decl.root.keys.iter().any(|key| key.name == index.name) {
            errors.push(SchemaError {
                code: SCHEMA_KEY_MEMBER_COLLISION,
                message: format!(
                    "identity key `{}` collides with index `{}`; identity keys and \
                     indexes share the store namespace",
                    index.name, index.name
                ),
                span: index.span,
            });
        }
        check_store_index_args(index, &decl.root.keys, resource, &mut errors);
    }

    (
        StoreSchema {
            root: decl.root.root.clone(),
            resource: decl.resource.clone(),
            docs: decl.docs.clone(),
            identity_keys: decl.root.keys.iter().map(key_def).collect(),
            indexes: decl.indexes.iter().map(index_schema).collect(),
        },
        errors,
    )
}

/// Report saved-data member rules for a resource attached by a split store
/// declaration. Concise `resource at ^root` runs these through
/// [`compile_resource`]; split stores call this from the store declaration so a
/// plain resource AST stays store-independent.
pub fn check_saved_member_rules(members: &[ResourceMember]) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    for member in members {
        check_member_unknown(member, &mut errors);
        check_member_keys(member, &mut errors);
    }
    errors
}

/// Compile a parsed enum into an [`EnumSchema`], with any errors.
///
/// Members flatten in pre-order DFS, so each member has a source traversal index.
/// Member-name uniqueness is per sibling level (two `tiger`s under one parent
/// collide; `Cat::tiger` and `Dog::tiger` do not), reported with the shared
/// duplicate-member code so it reads like a resource's. The `category` flag and
/// having children are held in lockstep:
/// a `category` with no children is dead, and a non-`category` with children is a
/// grouping node a value could never select — both are rejected, so every parent is
/// a category and every non-category is a leaf.
pub fn compile_enum(decl: &EnumDecl) -> (EnumSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();
    let mut members = Vec::new();
    flatten_enum_members(&decl.members, None, &mut members, &mut errors);
    let schema = EnumSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        members,
    };
    (schema, errors)
}

/// Append one sibling level to `members` in pre-order — each member before its
/// own children — recording its parent traversal index and recursing into its
/// nested members. A duplicate name at the same level is reported and dropped, so
/// the flattened tree reflects only the distinct members.
fn flatten_enum_members(
    siblings: &[EnumMember],
    parent: Option<usize>,
    members: &mut Vec<EnumMemberSchema>,
    errors: &mut Vec<SchemaError>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for member in siblings {
        if seen.contains(&member.name.as_str()) {
            errors.push(SchemaError {
                code: SCHEMA_DUPLICATE_MEMBER,
                message: format!("duplicate enum member `{}`", member.name),
                span: member.span,
            });
            continue;
        }
        seen.push(&member.name);
        if member.category && member.members.is_empty() {
            errors.push(SchemaError {
                code: SCHEMA_CATEGORY_LEAF,
                message: format!(
                    "category `{}` has no members; a category must group nested members",
                    member.name
                ),
                span: member.span,
            });
        } else if !member.category && !member.members.is_empty() {
            errors.push(SchemaError {
                code: SCHEMA_PARENT_NOT_CATEGORY,
                message: format!(
                    "`{}` has nested members but is not a category; mark a grouping member \
                     `category`, since a value selects a concrete member under it, not the \
                     group itself",
                    member.name
                ),
                span: member.span,
            });
        }
        let ordinal = members.len();
        members.push(EnumMemberSchema {
            name: member.name.clone(),
            docs: member.docs.clone(),
            parent,
            category: member.category,
        });
        flatten_enum_members(&member.members, Some(ordinal), members, errors);
    }
}

/// Validate a keyed-layer's own key parameters, descending into groups. A keyed
/// layer's key must be a saved key, so it may not embed `unknown` and may not be
/// an unorderable type such as `decimal`. A keyed leaf and a keyed group both
/// carry their key parameters in `keys`; an unkeyed field or group has none.
/// Identity keys are checked by store compilation.
fn check_member_keys(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            if field.keys.is_empty()
                && let Some((key, _)) = map_entry(&field.ty.text)
            {
                check_synthetic_map_key(key, field.span, errors);
            }
            check_key_params(&field.keys, field.span, errors);
        }
        ResourceMember::Group(group) => {
            check_key_params(&group.keys, group.span, errors);
            for nested in &group.members {
                check_member_keys(nested, errors);
            }
        }
    }
}

/// Report each key parameter whose type cannot be a saved key. Key params have
/// no span of their own, so errors point at the keyed layer's `span`.
fn check_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    for key in keys {
        if let Some(error) = unsupported_map_key_param_error(key, span) {
            errors.push(error);
            continue;
        }
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("key", &key.name, span));
        } else if let Some(error) = key_type_error("key", &key.name, &ty, span) {
            errors.push(error);
        }
    }
}

fn unsupported_map_key_param_error(key: &KeyParam, span: SourceSpan) -> Option<SchemaError> {
    unsupported_map_key_error(&key.name, &key.ty.text, span)
}

fn unsupported_map_key_error(name: &str, ty: &str, span: SourceSpan) -> Option<SchemaError> {
    contains_map_type_syntax(ty).then(|| SchemaError {
        code: SCHEMA_UNSUPPORTED_TYPE,
        message: format!(
            "key `{}` uses `map[...]`, which is only supported as saved \
             keyed-leaf member sugar",
            name
        ),
        span,
    })
}

fn check_synthetic_map_key(key: &str, span: SourceSpan, errors: &mut Vec<SchemaError>) {
    if let Some(error) = unsupported_map_key_error("key", key, span) {
        errors.push(error);
        return;
    }
    let ty = Type::resolve_text(key);
    if ty.embeds_unknown() {
        errors.push(unknown_error("key", "key", span));
    } else if let Some(error) = key_type_error("key", "key", &ty, span) {
        errors.push(error);
    }
}

/// Reject `unknown` on the value type of a field or keyed leaf, descending into
/// groups. A keyed layer's own key parameters are validated separately in
/// [`check_member_keys`].
fn check_member_unknown(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            // A keyed leaf carries its value type the same way a plain field
            // does; both reject `unknown`.
            let (what, ty) = if field.keys.is_empty()
                && let Some((_, value)) = map_entry(&field.ty.text)
            {
                ("keyed leaf", Type::resolve_text(value))
            } else if field.keys.is_empty() {
                ("field", Type::resolve(&field.ty))
            } else {
                ("keyed leaf", Type::resolve(&field.ty))
            };
            if ty.embeds_unknown() {
                errors.push(unknown_error(what, &field.name, field.span));
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_member_unknown(nested, errors);
            }
        }
    }
}

fn check_unsupported_map_types(
    members: &[ResourceMember],
    saved_map_sugar: bool,
    errors: &mut Vec<SchemaError>,
) {
    for member in members {
        match member {
            ResourceMember::Field(field) => {
                if !saved_map_sugar {
                    check_unsupported_map_key_params(&field.keys, field.span, errors);
                }
                if let Some((key, value)) = map_entry(&field.ty.text)
                    && saved_map_sugar
                    && field.keys.is_empty()
                {
                    if field.required || contains_map_type(value) {
                        errors.push(unsupported_map_field_error(field));
                    } else if !contains_map_type(key) {
                        continue;
                    }
                } else if contains_map_type(&field.ty.text) {
                    errors.push(unsupported_map_field_error(field));
                }
            }
            ResourceMember::Group(group) => {
                if !saved_map_sugar {
                    check_unsupported_map_key_params(&group.keys, group.span, errors);
                }
                check_unsupported_map_types(&group.members, saved_map_sugar, errors);
            }
        }
    }
}

fn check_unsupported_map_key_params(
    keys: &[KeyParam],
    span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    for key in keys {
        if let Some(error) = unsupported_map_key_param_error(key, span) {
            errors.push(error);
        }
    }
}

fn unsupported_map_field_error(field: &FieldDecl) -> SchemaError {
    SchemaError {
        code: SCHEMA_UNSUPPORTED_TYPE,
        message: format!(
            "field `{}` uses `map[...]`, which is only supported as unrequired \
             saved keyed-leaf member sugar",
            field.name
        ),
        span: field.span,
    }
}

fn is_supported_map_member(field: &FieldDecl) -> bool {
    !field.required
        && field.keys.is_empty()
        && map_entry(&field.ty.text)
            .map(|(key, value)| !contains_map_type(key) && !contains_map_type(value))
            .unwrap_or(false)
}

/// Apply the saved named-field rule directly to resource members. Split store
/// declarations use this after resolving the resource they attach.
pub fn check_saved_named_member_fields(
    members: &[ResourceMember],
    enums: &[String],
) -> Vec<SchemaError> {
    check_saved_named_member_fields_with(members, |name| {
        name.contains("::") || enums.iter().any(|enum_name| enum_name == name)
    })
}

/// Apply the saved named-field rule with a project-aware enum resolver. Schema
/// compilation only knows same-file enum names; the checker supplies a resolver
/// for qualified names after imports and module visibility are known.
pub fn check_saved_named_member_fields_with(
    members: &[ResourceMember],
    mut is_declared_enum_name: impl FnMut(&str) -> bool,
) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    for member in members {
        check_named_field(member, &mut is_declared_enum_name, &mut errors);
    }
    errors
}

fn check_named_field(
    member: &ResourceMember,
    is_declared_enum_name: &mut impl FnMut(&str) -> bool,
    errors: &mut Vec<SchemaError>,
) {
    match member {
        ResourceMember::Field(field) => {
            let ty = if contains_map_type(&field.ty.text) {
                if field.keys.is_empty()
                    && let Some((_, value)) = map_entry(&field.ty.text)
                    && is_supported_map_member(field)
                    && !contains_map_type(value)
                {
                    Type::resolve_text(value)
                } else {
                    return;
                }
            } else {
                Type::resolve(&field.ty)
            };
            check_named_field_type(&ty, field, is_declared_enum_name, errors);
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_named_field(nested, is_declared_enum_name, errors);
            }
        }
    }
}

fn check_named_field_type(
    ty: &Type,
    field: &FieldDecl,
    is_declared_enum_name: &mut impl FnMut(&str) -> bool,
    errors: &mut Vec<SchemaError>,
) {
    match ty {
        Type::Named(name) if !is_declared_enum_name(name) => errors.push(SchemaError {
            code: SCHEMA_NON_ENUM_NAMED_FIELD,
            message: format!(
                "saved field `{}` has type `{name}`, which is not a declared enum; \
                 a saved field stores a scalar or checked enum value",
                field.name
            ),
            span: field.span,
        }),
        Type::Sequence(element) => {
            check_named_field_type(element, field, is_declared_enum_name, errors);
        }
        _ => {}
    }
}

fn check_store_index_args(
    index: &IndexDecl,
    keys: &[KeyParam],
    resource: &ResourceSchema,
    errors: &mut Vec<SchemaError>,
) {
    for arg in &index.args {
        match store_index_arg_type(arg, keys, resource) {
            None if store_index_arg_is_nested_field(arg, resource) => {
                errors.push(SchemaError {
                    code: SCHEMA_NESTED_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` names a field nested through an \
                         unkeyed group, which the write planner does not maintain",
                        index.name
                    ),
                    span: index.span,
                });
            }
            None => errors.push(SchemaError {
                code: SCHEMA_UNKNOWN_INDEX_ARG,
                message: format!(
                    "index `{}` argument `{arg}` does not name an identity \
                     key or top-level field",
                    index.name
                ),
                span: index.span,
            }),
            Some(ty) => {
                if let Some(error) = index_arg_type_key_error(&index.name, arg, &ty, index.span) {
                    errors.push(error);
                }
            }
        }
    }
    if !index.unique && !ends_with_identity_keys(&index.args, keys) {
        errors.push(SchemaError {
            code: SCHEMA_INDEX_MISSING_IDENTITY_KEYS,
            message: format!(
                "non-unique index `{}` must end with all identity key(s) in \
                 declaration order so each entry is distinct",
                index.name
            ),
            span: index.span,
        });
    }
}

fn store_index_arg_type(arg: &str, keys: &[KeyParam], resource: &ResourceSchema) -> Option<Type> {
    if arg.contains('.') {
        return None;
    }
    if let Some(key) = keys.iter().find(|key| key.name == arg) {
        return Some(Type::resolve(&key.ty));
    }
    resource.field_type(&[arg]).cloned()
}

fn store_index_arg_is_nested_field(arg: &str, resource: &ResourceSchema) -> bool {
    if arg.contains('.') {
        let segments: Vec<&str> = arg.split('.').collect();
        return resource.field_type(&segments).is_some();
    }
    resource
        .members
        .iter()
        .any(|node| node_has_nested_field_named(node, arg))
}

fn node_has_nested_field_named(node: &Node, name: &str) -> bool {
    if !node.key_params.is_empty() {
        return false;
    }
    matches!(node.kind, NodeKind::Group)
        && node.members.iter().any(|member| {
            (member.name == name && member.is_plain_field())
                || node_has_nested_field_named(member, name)
        })
}

/// Does this index argument list end with all identity key names in declaration
/// order? A non-unique entry is a presence marker, so without the trailing
/// identity keys two records sharing the indexed values collapse onto one entry.
fn ends_with_identity_keys(args: &[String], keys: &[KeyParam]) -> bool {
    args.len() >= keys.len()
        && args[args.len() - keys.len()..]
            .iter()
            .zip(keys)
            .all(|(arg, key)| arg == &key.name)
}

/// The element type spelling of a `sequence[T]`, or `None` for a non-sequence
/// type. The one place the `sequence[...]` spelling is parsed; [`Type::resolve`]
/// drives off it. `sequence[T]` is sugar for the 1-based `pos: int` keyed tree.
fn sequence_element(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
        .map(str::trim)
}

/// The key and value type spellings of a `map[K, V]`, split at the top-level
/// comma, or `None` for a non-map type.
fn map_entry(text: &str) -> Option<(&str, &str)> {
    let inner = text
        .trim()
        .strip_prefix("map[")
        .and_then(|rest| rest.strip_suffix(']'))?;
    split_top_level_comma(inner).map(|(key, value)| (key.trim(), value.trim()))
}

/// Whether a type spelling contains the unsupported `map[...]` type form.
pub fn contains_map_type_syntax(text: &str) -> bool {
    contains_map_type(text)
}

fn contains_map_type(text: &str) -> bool {
    let text = text.trim();
    map_entry(text).is_some()
        || sequence_element(text)
            .map(contains_map_type)
            .unwrap_or(false)
}

fn split_top_level_comma(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '[' => depth = depth.checked_add(1)?,
            ']' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                let key = &text[..index];
                let value = &text[index + ch.len_utf8()..];
                return (!key.is_empty() && !value.is_empty()).then_some((key, value));
            }
            _ => {}
        }
    }
    None
}

fn unknown_error(what: &str, name: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        code: SCHEMA_UNKNOWN_IN_SAVED,
        message: format!(
            "saved {what} `{name}` cannot use `unknown`; managed saved \
             schemas use concrete types"
        ),
        span,
    }
}

/// Why a type may not be a saved key, or `Ok` when it is a valid one. Saved keys
/// project from an orderable scalar value, so the rule is an allowlist: every
/// scalar except `decimal` is a key; everything else is rejected. The verdict
/// needs no knowledge of what a name refers to, so a local enum, a cross-module
/// enum, a resource, a typo, and a store identity are all the same
/// `NonScalar` case, caught structurally without an enum or resource list.
enum KeyTypeVerdict {
    Ok,
    /// `decimal` — a scalar, but the one with no order-preserving key encoding.
    Decimal,
    /// An identity, a name, or a sequence, none of which projects to an
    /// orderable scalar key.
    NonScalar,
}

/// Classify a key type. `decimal` is the one scalar the store cannot encode as a
/// key; every other scalar is orderable. A store identity, name, or sequence
/// is a non-scalar key.
fn classify_key_type(ty: &Type) -> KeyTypeVerdict {
    match ty {
        Type::Scalar(ScalarType::Decimal) => KeyTypeVerdict::Decimal,
        Type::Scalar(_) => KeyTypeVerdict::Ok,
        Type::Identity(_) | Type::Named(_) | Type::Sequence(_) | Type::Unknown => {
            KeyTypeVerdict::NonScalar
        }
    }
}

/// The error a key of type `ty` earns in an identity-key or keyed-layer position,
/// or `None` if it is a valid key. `decimal` keeps its own "no key encoding"
/// message and code; any other non-scalar is the orderable-scalar rule. `unknown`
/// is reported separately by the caller, so it does not reach here.
fn key_type_error(what: &str, name: &str, ty: &Type, span: SourceSpan) -> Option<SchemaError> {
    match classify_key_type(ty) {
        KeyTypeVerdict::Ok => None,
        KeyTypeVerdict::Decimal => Some(SchemaError {
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "saved {what} `{name}` cannot use `decimal`; saved keys use ordered \
                 key types and `decimal` has no key encoding"
            ),
            span,
        }),
        KeyTypeVerdict::NonScalar => Some(SchemaError {
            code: SCHEMA_NONSCALAR_KEY,
            message: format!(
                "saved {what} `{name}` must be an orderable scalar type, but found `{ty}`"
            ),
            span,
        }),
    }
}

fn index_arg_type_key_error(
    index: &str,
    arg: &str,
    resolved: &Type,
    span: SourceSpan,
) -> Option<SchemaError> {
    match resolved {
        Type::Scalar(ScalarType::Decimal) => Some(SchemaError {
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "index `{index}` argument `{arg}` is a `decimal`, which has no key \
                 encoding; index arguments use ordered key types"
            ),
            span,
        }),
        Type::Scalar(_) | Type::Named(_) => None,
        Type::Sequence(_) | Type::Identity(_) | Type::Unknown => Some(SchemaError {
            code: SCHEMA_NONSCALAR_KEY,
            message: format!(
                "index `{index}` argument `{arg}` must be an orderable scalar type, \
                 but found `{resolved}`"
            ),
            span,
        }),
    }
}

/// Compile the members nested inside a group into nodes.
fn group_members(group: &GroupDecl, errors: &mut Vec<SchemaError>, map_sugar: bool) -> Vec<Node> {
    let mut members = Vec::new();
    let mut names = Namespace::default();

    for member in &group.members {
        names.check(member_name(member), member_span(member), errors);
        members.push(member_node(member, errors, map_sugar));
    }

    members
}

/// The name of a resource member.
fn member_name(member: &ResourceMember) -> &str {
    match member {
        ResourceMember::Field(field) => &field.name,
        ResourceMember::Group(group) => &group.name,
    }
}

/// The span of a resource member.
fn member_span(member: &ResourceMember) -> SourceSpan {
    match member {
        ResourceMember::Field(field) => field.span,
        ResourceMember::Group(group) => group.span,
    }
}

/// Compile one resource member into a [`Node`]:
/// an unkeyed plain field is a top-level `Slot`; `sequence[T]`, `map[K, V]`,
/// and keyed fields are keyed-leaf `Slot`s; a group is a `Group` with recursed
/// members.
fn member_node(member: &ResourceMember, errors: &mut Vec<SchemaError>, map_sugar: bool) -> Node {
    match member {
        ResourceMember::Field(field) if field.keys.is_empty() => {
            // Collection member sugar becomes a keyed `Slot` rather than a
            // plain top-level field.
            match Type::resolve(&field.ty) {
                Type::Sequence(element) => sequence_leaf(field, *element),
                ty => {
                    if map_sugar && let Some((key, value)) = map_entry(&field.ty.text) {
                        map_leaf(field, key, value)
                    } else {
                        slot_node(field, ty, vec![], field.required)
                    }
                }
            }
        }
        // A keyed field is a keyed-leaf layer; its declared type is the leaf type
        // and a keyed leaf never exposes `required`.
        ResourceMember::Field(field) => {
            check_duplicate_key_params(&field.keys, field.span, errors);
            slot_node(
                field,
                Type::resolve(&field.ty),
                field.keys.iter().map(key_def).collect(),
                false,
            )
        }
        ResourceMember::Group(group) => {
            check_duplicate_key_params(&group.keys, group.span, errors);
            Node {
                name: group.name.clone(),
                docs: group.docs.clone(),
                key_params: group.keys.iter().map(key_def).collect(),
                members: group_members(group, errors, map_sugar),
                kind: NodeKind::Group,
            }
        }
    }
}

/// A `Slot` node for `field`, carrying its value type, key parameters (empty for
/// a plain field, the keyed-leaf keys otherwise), and required flag.
fn slot_node(field: &FieldDecl, ty: Type, key_params: Vec<KeyDef>, required: bool) -> Node {
    Node {
        name: field.name.clone(),
        docs: field.docs.clone(),
        key_params,
        members: Vec::new(),
        kind: NodeKind::Slot { ty, required },
    }
}

/// Desugar `name: sequence[T]` into the keyed leaf `name(pos: int): T`. The
/// implicit `pos: int` key matches the canonical sequence spelling, so the
/// resulting node is identical to the one `name(pos: int): T` produces and
/// append/read/traverse work unchanged.
fn sequence_leaf(field: &FieldDecl, element: Type) -> Node {
    slot_node(
        field,
        element,
        vec![KeyDef {
            name: "pos".to_string(),
            ty: Type::Scalar(ScalarType::Int),
        }],
        false,
    )
}

/// Desugar `name: map[K, V]` into the keyed leaf `name(key: K): V`.
fn map_leaf(field: &FieldDecl, key: &str, value: &str) -> Node {
    slot_node(
        field,
        Type::resolve_text(value),
        vec![KeyDef {
            name: "key".to_string(),
            ty: Type::resolve_text(key),
        }],
        false,
    )
}

/// Report a keyed layer's key parameters that repeat a name. Key params share a
/// per-layer namespace; two keys of the same name are unaddressable. Key params
/// have no span of their own, so errors point at the layer's `span`.
fn check_duplicate_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    let mut seen: Vec<&str> = Vec::new();
    for key in keys {
        if seen.contains(&key.name.as_str()) {
            errors.push(duplicate_key_error(&key.name, span));
        } else {
            seen.push(&key.name);
        }
    }
}

fn duplicate_key_error(name: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        code: SCHEMA_DUPLICATE_MEMBER,
        message: format!("duplicate key `{name}`"),
        span,
    }
}

fn index_schema(index: &IndexDecl) -> IndexSchema {
    IndexSchema {
        name: index.name.clone(),
        docs: index.docs.clone(),
        args: index.args.clone(),
        unique: index.unique,
    }
}

fn key_def(key: &KeyParam) -> KeyDef {
    KeyDef {
        name: key.name.clone(),
        ty: Type::resolve(&key.ty),
    }
}

/// Tracks member names seen at one nesting level so duplicates can be reported.
/// Fields, layers, and indexes share one flat namespace per level.
#[derive(Default)]
struct Namespace {
    seen: Vec<String>,
}

impl Namespace {
    fn check(&mut self, name: &str, span: SourceSpan, errors: &mut Vec<SchemaError>) {
        if self.seen.iter().any(|existing| existing == name) {
            errors.push(SchemaError {
                code: SCHEMA_DUPLICATE_MEMBER,
                message: format!("duplicate resource member `{name}`"),
                span,
            });
        } else {
            self.seen.push(name.to_string());
        }
    }
}
