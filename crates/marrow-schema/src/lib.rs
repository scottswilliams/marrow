//! Compiles a parsed Marrow resource declaration into a typed-tree
//! [`ResourceSchema`].
//!
//! It maps the parsed resource declaration produced by `marrow-syntax` onto the
//! saved/local tree shape: a saved root with identity keys, top-level fields,
//! keyed layers (sequences, keyed trees, groups, and history), and declared
//! indexes. Semantic validation beyond structure is deferred; see the notes on
//! [`compile_resource`].

use std::fmt;

use marrow_syntax::{
    EnumDecl, EnumMember, FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember,
    SavedRoot, SourceSpan, TypeRef,
};

pub mod stdlib;

// The canonical scalar type lives in marrow-store; re-export it so resolution
// and downstream crates share one import path for the storable scalars.
pub use marrow_store::value::ScalarType;

/// A type annotation resolved once during schema compilation, so downstream
/// crates match on structure instead of re-parsing the source spelling.
///
/// Resolution is structural and module-blind: it decides everything a single
/// declaration can (a scalar, a `sequence[...]`, an `X::Id` identity, `unknown`),
/// and leaves any other bare or qualified name as [`Type::Named`]. The checker,
/// which knows the project's resource names, promotes a `Named` to a resource
/// reference or flags it unknown; the runtime only ever reads the scalar leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Scalar(ScalarType),
    Sequence(Box<Type>),
    /// A resource identity such as `Book::Id`, carrying the resource name.
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
        // A resource identity such as `Book::Id` names the resource it wraps.
        if let Some(resource) = text.strip_suffix("::Id") {
            return Self::Identity(resource.to_string());
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

    /// The scalar a stored field of this type encodes as: a plain scalar's own
    /// type, or `int` for an enum field, whose value is the selected member's
    /// declaration-order ordinal. A saved field that is a bare [`Type::Named`] is
    /// always an enum: [`check_saved_named_fields`] rejects any other bare name
    /// (an undefined name or a resource type) at compile time, so by the time a
    /// `Named` field reaches the store it stores its ordinal as an `int`. The
    /// storage boundary — value type-checks, field reads, whole-resource reads —
    /// uses this.
    pub fn stored_scalar(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            Self::Named(_) => Some(ScalarType::Int),
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
            Self::Identity(resource) => write!(f, "{resource}::Id"),
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
/// that want only one kind filter `members` by [`Element`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub saved_root: Option<SavedRootSchema>,
    pub members: Vec<Node>,
    pub indexes: Vec<IndexSchema>,
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
    /// differ only in whether the terminal name is a field (a [`Element::Slot`]) or
    /// a group (a [`Element::Group`]) to descend, so both share the one walk.
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
        match &self.descend_layers(layers)?.element {
            Element::Slot { ty, .. } => Some(ty),
            Element::Group => None,
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
    members.iter().find_map(|node| match &node.element {
        Element::Slot { ty, .. } if node.name == name && node.key_params.is_empty() => Some(ty),
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
        self.key_params.is_empty() && matches!(self.element, Element::Slot { .. })
    }

    /// The type of this node when it is a plain field, else `None`. Lets a caller
    /// select plain fields and bind their type in one pass.
    pub fn plain_field_type(&self) -> Option<&Type> {
        match &self.element {
            Element::Slot { ty, .. } if self.key_params.is_empty() => Some(ty),
            _ => None,
        }
    }
}

/// The saved root a resource is attached to, with the identity keys that live
/// in the saved path. Identity keys are not stored fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRootSchema {
    pub root: String,
    pub identity_keys: Vec<KeyDef>,
}

impl SavedRootSchema {
    /// Does this saved root qualify for the default `nextId` allocation policy?
    /// Only a resource with exactly one `int` identity key does; composite
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

/// A named, typed key parameter of a saved root or keyed layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDef {
    pub name: String,
    pub ty: Type,
}

/// One node of the resource tree: a top-level field, a keyed leaf, or a group,
/// distinguished by its [`Element`]. The recursive `members` are filled only for
/// a group; a keyed leaf carries `key_params` and an empty `members`; a
/// top-level field carries neither key params nor members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    /// Empty for a top-level field; non-empty for any keyed leaf or keyed group.
    pub key_params: Vec<KeyDef>,
    /// Empty for any [`Element::Slot`]; the nested nodes for an [`Element::Group`].
    pub members: Vec<Node>,
    pub element: Element,
}

/// What a [`Node`] holds: a scalar value (`Slot`) or nested members (`Group`).
///
/// A top-level field and a keyed-leaf layer are both `Slot`s — the keyed leaf is
/// a `Slot` with non-empty `key_params`. A group (`notes(noteId: string)` /
/// `versions(version)` / an unkeyed `name`) is a `Group` with nested `members`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
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
    pub stable_id: Option<String>,
}

/// The compiled form of an enum: a named, fixed set of members. Members are held
/// flat in pre-order DFS, so a member's index is its stored ordinal; the tree
/// shape lives in each member's `parent` link. A flat enum is the degenerate
/// one-level tree — every member at the top level, in source order — so its
/// compiled form is byte-identical to a non-hierarchical enum and existing data
/// needs no migration. An enum is its own construct, not a [`ResourceSchema`]; it
/// owns no saved root and stores as the ordinal of the selected member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumSchema {
    pub name: String,
    pub docs: Vec<String>,
    /// Members in pre-order DFS; a member's index is its stored ordinal.
    pub members: Vec<EnumMemberSchema>,
}

/// One enum member. `parent` is the ordinal of the enclosing member, `None` at the
/// top level. A `category` member groups its descendants and is not selectable as a
/// value. `stable_id` is a reserved slot for the rename-safe stable-id work; it is
/// unused while ordinals are positional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMemberSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub parent: Option<usize>,
    pub category: bool,
}

/// The outcome of walking a relative member path against an [`EnumSchema`]. The
/// one walk behind value, `is`, and `match` arm resolution returns this so each
/// caller applies its own position rule (selectability) to a single resolved
/// member and reports ambiguity with the same actionable wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberPathResolution {
    /// The path names exactly this member ordinal.
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
    /// The stored ordinal of `member` — its index in pre-order DFS — or `None` if
    /// the enum has no such member. When two members at different levels share a
    /// bare name, the first in pre-order wins; the checker rejects an ambiguous
    /// reference before this is reached for a value or arm.
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

    /// Whether the member at `ordinal` has the given name.
    fn member_is(&self, ordinal: usize, name: &str) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.name == name)
    }

    /// The ordinal of the child of `parent` named `name`, if any. Children are the
    /// members whose `parent` link is `parent`; sibling names are unique, so at most
    /// one matches.
    fn child_named(&self, parent: usize, name: &str) -> Option<usize> {
        (0..self.members.len()).find(|&ordinal| {
            self.members[ordinal].parent == Some(parent) && self.members[ordinal].name == name
        })
    }

    /// The member name an ordinal selects, or `None` if it is out of range.
    pub fn member_name(&self, ordinal: usize) -> Option<&str> {
        self.members.get(ordinal).map(|m| m.name.as_str())
    }

    /// Whether `ordinal` sits at or under `ancestor` in the member tree — the
    /// `is` primitive. Inclusive: a member is its own descendant, so a concrete
    /// leaf on both sides is exact equality and a category ancestor matches its
    /// whole subtree. Walks `parent` links up from `ordinal`.
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

    /// Whether the member at `ordinal` is a category — a grouping node that is not
    /// selectable as a value but may name a whole subtree in `match` or `is`.
    pub fn is_category(&self, ordinal: usize) -> bool {
        self.members.get(ordinal).is_some_and(|m| m.category)
    }

    /// The ordinals at or under `ancestor` in pre-order, inclusive — the members a
    /// category arm or an `is` test covers.
    pub fn subtree_ordinals(&self, ancestor: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_descendant(ordinal, ancestor))
    }

    /// The ordinals a value can actually hold: the concrete (non-category) leaves.
    /// A category is never selectable, and a member with children is a grouping
    /// node whose value is one of its descendants, so only childless non-category
    /// members are selectable.
    pub fn selectable_leaves(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.members.len()).filter(move |&ordinal| self.is_selectable_leaf(ordinal))
    }

    /// Whether `ordinal` is a selectable leaf: concrete (not a category) and with
    /// no children.
    pub fn is_selectable_leaf(&self, ordinal: usize) -> bool {
        let Some(member) = self.members.get(ordinal) else {
            return false;
        };
        !member.category && !self.has_children(ordinal)
    }

    /// Whether any member names `ordinal` as its parent.
    pub fn has_children(&self, ordinal: usize) -> bool {
        self.members.iter().any(|m| m.parent == Some(ordinal))
    }

    /// The dotted path of names from the root to `ordinal` (`["tiger", "bengal"]`),
    /// for diagnostics. Empty when the ordinal is out of range.
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

/// A top-level field or layer shares a name with an identity key. Identity keys
/// live in the saved path, so a stored member of the same name is ambiguous.
pub const SCHEMA_KEY_MEMBER_COLLISION: &str = "schema.key_member_collision";

/// An index argument does not resolve to an identity key or a top-level field.
/// Index arguments do not walk keyed child layers or unkeyed group descendants.
pub const SCHEMA_UNKNOWN_INDEX_ARG: &str = "schema.unknown_index_arg";

/// Two resource elements declare the same stable ID. Stable IDs must be unique.
pub const SCHEMA_DUPLICATE_STABLE_ID: &str = "schema.duplicate_stable_id";

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

/// An index is declared on a resource with no keyed saved root. Declared indexes
/// are members of keyed saved resources; a singleton (keyless) or local
/// (non-saved) resource has no generated identity for an entry to point to.
pub const SCHEMA_INDEX_REQUIRES_KEYED_ROOT: &str = "schema.index_requires_keyed_root";

/// An index argument names a field nested through an unkeyed group. The write
/// planner matches index arguments by flat top-level name, so it would silently
/// never maintain such an entry. Until nested index resolution lands, reject it.
pub const SCHEMA_NESTED_INDEX_ARG: &str = "schema.nested_index_arg";

/// A managed saved field's type is a bare name that is not a declared enum. A
/// saved field stores a scalar; a bare name reaches the store only as an enum,
/// whose value is its member ordinal. An undefined name or a resource type has
/// no stored scalar form, so it cannot be a saved field.
pub const SCHEMA_NON_ENUM_NAMED_FIELD: &str = "schema.non_enum_named_field";

/// A saved key (an identity key, a keyed-layer key parameter, or an index
/// argument) is typed as a non-scalar. A key must be an orderable scalar, because
/// the store projects a key from its scalar value and the key guard cannot tell
/// a member ordinal from a raw string or int written into a non-scalar position.
/// Every bare or qualified name — a local enum, a cross-module enum, a resource,
/// or a typo — every sequence, and every resource identity is rejected
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
/// keep checking. Maps structure and the single-resource rules the schema alone
/// can decide: `unknown` is rejected in managed saved fields and
/// keys, an identity key may not share a name with a top-level member, index
/// arguments must resolve within the resource, and stable IDs are unique within
/// the resource.
///
/// Deferred: full type validation, one-owner-per-root, and project-wide
/// (cross-resource) stable-ID uniqueness.
pub fn compile_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();

    let saved_root = decl.store.as_ref().map(|store| SavedRootSchema {
        root: store.root.clone(),
        identity_keys: store.keys.iter().map(key_def).collect(),
    });

    let mut members = Vec::new();
    let mut indexes = Vec::new();
    let mut names = Namespace::default();

    for member in &decl.members {
        match member {
            ResourceMember::Index(index) => {
                names.check(&index.name, index.span, &mut errors);
                indexes.push(index_schema(index));
            }
            _ => {
                names.check(member_name(member), member_span(member), &mut errors);
                members.push(member_node(member, &mut errors));
            }
        }
    }

    let schema = ResourceSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        saved_root,
        members,
        indexes,
    };

    // Saved-data rules apply only to managed saved resources. They are reported
    // over the declaration, which carries the spans the built schema does not.
    if let Some(store) = &decl.store {
        check_saved_data(store, &decl.members, decl.span, &mut errors);
    }

    check_index_args(decl, &mut errors);
    check_stable_ids(decl, &mut errors);

    (schema, errors)
}

/// Compile a parsed enum into an [`EnumSchema`], with any errors.
///
/// Members flatten in pre-order DFS, so a member's index is its stored ordinal and
/// a flat enum keeps its 0..n ordinals byte-identical. Member-name uniqueness is
/// per sibling level (two `tiger`s under one parent collide; `Cat::tiger` and
/// `Dog::tiger` do not), reported with the shared duplicate-member code so it reads
/// like a resource's. The `category` flag and having children are held in lockstep:
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
/// own children — recording its `parent` ordinal and recursing into its nested
/// members. A duplicate name at the same level is reported and dropped, so the
/// stored members and their ordinals reflect only the distinct ones.
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
            stable_id: member.stable_id.clone(),
            parent,
            category: member.category,
        });
        flatten_enum_members(&member.members, Some(ordinal), members, errors);
    }
}

/// Report the saved-data rules for a managed saved resource: reject `unknown`
/// in identity keys, fields, and keyed leaves (recursively), and reject an
/// identity key that shares a name with a top-level member.
///
/// Errors are collected in source order: identity keys, then members. Identity
/// keys have no span of their own, so their errors point at the declaration.
fn check_saved_data(
    store: &SavedRoot,
    members: &[ResourceMember],
    decl_span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    check_duplicate_key_params(&store.keys, decl_span, errors);
    for key in &store.keys {
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("identity key", &key.name, decl_span));
        } else if let Some(error) = key_type_error("identity key", &key.name, &ty, decl_span) {
            errors.push(error);
        }
        if let Some(span) = top_level_member_span(members, &key.name) {
            errors.push(SchemaError {
                code: SCHEMA_KEY_MEMBER_COLLISION,
                message: format!(
                    "identity key `{}` collides with a top-level member of the \
                     same name; identity keys live in the saved path, not stored \
                     members",
                    key.name
                ),
                span,
            });
        }
    }

    for member in members {
        check_member_unknown(member, errors);
        check_member_keys(member, errors);
    }
}

/// Validate a keyed-layer's own key parameters, descending into groups. A keyed
/// layer's key must be a saved key, so it may not embed `unknown` and may not be
/// an unorderable type such as `decimal`. A keyed leaf and a keyed group both
/// carry their key parameters in `keys`; an unkeyed field or group has none.
/// Identity keys are checked separately in [`check_saved_data`].
fn check_member_keys(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => check_key_params(&field.keys, field.span, errors),
        ResourceMember::Group(group) => {
            check_key_params(&group.keys, group.span, errors);
            for nested in &group.members {
                check_member_keys(nested, errors);
            }
        }
        ResourceMember::Index(_) => {}
    }
}

/// Report each key parameter whose type cannot be a saved key. Key params have
/// no span of their own, so errors point at the keyed layer's `span`.
fn check_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    for key in keys {
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("key", &key.name, span));
        } else if let Some(error) = key_type_error("key", &key.name, &ty, span) {
            errors.push(error);
        }
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
            let what = if field.keys.is_empty() {
                "field"
            } else {
                "keyed leaf"
            };
            if Type::resolve(&field.ty).embeds_unknown() {
                errors.push(unknown_error(what, &field.name, field.span));
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_member_unknown(nested, errors);
            }
        }
        ResourceMember::Index(_) => {}
    }
}

/// Reject every managed saved field whose type is a bare name that is not one of
/// `enums`. A bare [`Type::Named`] reaches a stored scalar only as an enum (its
/// member ordinal); an undefined name or a resource type has no stored scalar
/// form. The caller resolves enum names cross-declaration and passes them here,
/// since [`compile_resource`] compiles one resource without that context. A
/// local (non-saved) resource is exempt — only saved fields lower into the store.
pub fn check_saved_named_fields(decl: &ResourceDecl, enums: &[String]) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    if decl.store.is_some() {
        for member in &decl.members {
            check_named_field(member, enums, &mut errors);
        }
    }
    errors
}

fn check_named_field(member: &ResourceMember, enums: &[String], errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            if let Type::Named(name) = Type::resolve(&field.ty)
                && !enums.iter().any(|enum_name| enum_name == &name)
            {
                errors.push(SchemaError {
                    code: SCHEMA_NON_ENUM_NAMED_FIELD,
                    message: format!(
                        "saved field `{}` has type `{name}`, which is not a declared enum; \
                         a saved field stores a scalar or an enum ordinal",
                        field.name
                    ),
                    span: field.span,
                });
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_named_field(nested, enums, errors);
            }
        }
        ResourceMember::Index(_) => {}
    }
}

/// The span of a top-level member named `name`, if one exists. Identity keys,
/// fields, layers, and index names share the resource namespace, so an identity
/// key may not reuse any of them.
fn top_level_member_span(members: &[ResourceMember], name: &str) -> Option<SourceSpan> {
    members
        .iter()
        .find(|member| member_name(member) == name)
        .map(member_span)
}

/// Resolve each index argument against the resource. Implemented indexes may
/// name an identity key or a top-level unkeyed field. Nested scalar fields
/// reached through unkeyed groups are recognized only to report the unsupported
/// nested-index diagnostic; indexes do not walk keyed child layers. Each
/// unresolved argument is reported at its index's span, in index then argument
/// order.
///
/// An index also requires a keyed saved root: a singleton (keyless) or local
/// (non-saved) resource has no identity for an entry to point to, which is
/// reported once per index and short-circuits the per-argument checks.
fn check_index_args(decl: &ResourceDecl, errors: &mut Vec<SchemaError>) {
    let keys = decl
        .store
        .as_ref()
        .map(|store| &store.keys[..])
        .unwrap_or(&[]);
    for member in &decl.members {
        let ResourceMember::Index(index) = member else {
            continue;
        };
        if keys.is_empty() {
            errors.push(SchemaError {
                code: SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
                message: format!(
                    "index `{}` requires a keyed saved root; a singleton or local \
                     resource has no identity for an index entry to point to",
                    index.name
                ),
                span: index.span,
            });
            continue;
        }
        for arg in &index.args {
            match index_arg_type(arg, keys, &decl.members) {
                None if has_nested_unkeyed_field_named(arg, &decl.members) => {
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
                // A dotted argument resolves through an unkeyed group, which the
                // write planner does not maintain.
                Some(_) if arg.contains('.') => errors.push(SchemaError {
                    code: SCHEMA_NESTED_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` names a field nested through an \
                         unkeyed group, which the write planner does not maintain",
                        index.name
                    ),
                    span: index.span,
                }),
                Some(ty) => {
                    if let Some(error) = index_arg_key_error(&index.name, arg, ty, index.span) {
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

/// The type `arg` resolves to in this resource, or `None` if it resolves to
/// nothing. A single segment may name an identity key or a top-level unkeyed
/// scalar field. A dotted path walks unkeyed groups (each non-final segment an
/// unkeyed group, the final segment a scalar unkeyed field); identity keys are
/// single-segment only.
fn index_arg_type<'a>(
    arg: &str,
    keys: &'a [KeyParam],
    members: &'a [ResourceMember],
) -> Option<&'a TypeRef> {
    let segments: Vec<&str> = arg.split('.').collect();
    if segments.len() == 1
        && let Some(key) = keys.iter().find(|key| key.name == segments[0])
    {
        return Some(&key.ty);
    }
    resolve_field_type(&segments, members)
}

/// Resolve a non-empty field path against `members` to its field type. The final
/// segment must be an unkeyed scalar field; every earlier segment must be an
/// unkeyed group whose members resolve the rest. Keyed fields and groups are
/// keyed layers that index arguments do not walk.
fn resolve_field_type<'a>(segments: &[&str], members: &'a [ResourceMember]) -> Option<&'a TypeRef> {
    let (name, rest) = segments.split_first().expect("non-empty field path");
    members.iter().find_map(|member| match member {
        ResourceMember::Field(field)
            if rest.is_empty() && field.name == *name && field.keys.is_empty() =>
        {
            Some(&field.ty)
        }
        ResourceMember::Group(group)
            if !rest.is_empty() && group.keys.is_empty() && group.name == *name =>
        {
            resolve_field_type(rest, &group.members)
        }
        _ => None,
    })
}

/// Does a bare index argument name a scalar field below at least one unkeyed
/// group? The argument is still unsupported, but it is a nested-field request
/// rather than an unknown name.
fn has_nested_unkeyed_field_named(name: &str, members: &[ResourceMember]) -> bool {
    !name.contains('.')
        && members.iter().any(|member| match member {
            ResourceMember::Group(group) if group.keys.is_empty() => {
                has_unkeyed_field_named(name, &group.members)
            }
            _ => false,
        })
}

fn has_unkeyed_field_named(name: &str, members: &[ResourceMember]) -> bool {
    members.iter().any(|member| match member {
        ResourceMember::Field(field) => field.keys.is_empty() && field.name == name,
        ResourceMember::Group(group) if group.keys.is_empty() => {
            has_unkeyed_field_named(name, &group.members)
        }
        _ => false,
    })
}

/// Report stable IDs that repeat within this resource. Stable IDs must be
/// unique; the later element is the error. Elements are visited in source order,
/// descending into each group before the next sibling, so the first occurrence
/// wins deterministically.
///
/// This covers the within-resource subset only; cross-resource uniqueness is
/// deferred to a later project-wide pass.
fn check_stable_ids(decl: &ResourceDecl, errors: &mut Vec<SchemaError>) {
    let mut seen: Vec<String> = Vec::new();
    for (id, span) in stable_ids(decl) {
        if seen.contains(&id) {
            errors.push(SchemaError {
                code: SCHEMA_DUPLICATE_STABLE_ID,
                message: format!("duplicate stable id `{id}`"),
                span,
            });
        } else {
            seen.push(id);
        }
    }
}

/// Every stable ID declared in a resource, paired with the span of the element
/// that carries it, in declaration order (descending into a group before the
/// next sibling). Drives within-resource uniqueness here and project-wide
/// uniqueness in the checker. Repeats are kept so callers can report them.
pub fn stable_ids(decl: &ResourceDecl) -> Vec<(String, SourceSpan)> {
    let mut ids = Vec::new();
    collect_stable_ids(&decl.members, &mut ids);
    ids
}

fn collect_stable_ids(members: &[ResourceMember], ids: &mut Vec<(String, SourceSpan)>) {
    for member in members {
        let (stable_id, span) = match member {
            ResourceMember::Field(field) => (&field.stable_id, field.span),
            ResourceMember::Group(group) => (&group.stable_id, group.span),
            ResourceMember::Index(index) => (&index.stable_id, index.span),
        };
        if let Some(id) = stable_id {
            ids.push((id.clone(), span));
        }
        if let ResourceMember::Group(group) = member {
            collect_stable_ids(&group.members, ids);
        }
    }
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
/// enum, a resource, a typo, and a resource identity are all the same
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
/// key; every other scalar is orderable. A resource identity, name, or sequence
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

/// The error an index argument of source type `ty` earns, or `None` if it is a
/// valid index key. An index entry keys on the argument's *stored* scalar, so the
/// orderability rule reads that projection: an enum field stores its ordinal as an
/// orderable `int` and indexes fine, while a `decimal` (no key encoding) and a
/// `sequence` (no single scalar) cannot. A resource identity has no supported
/// index-key projection yet, so it is rejected with the other non-scalars.
fn index_arg_key_error(
    index: &str,
    arg: &str,
    ty: &TypeRef,
    span: SourceSpan,
) -> Option<SchemaError> {
    let resolved = Type::resolve(ty);
    match resolved.stored_scalar() {
        Some(ScalarType::Decimal) => Some(SchemaError {
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "index `{index}` argument `{arg}` is a `decimal`, which has no key \
                 encoding; index arguments use ordered key types"
            ),
            span,
        }),
        Some(_) => None,
        None => Some(SchemaError {
            code: SCHEMA_NONSCALAR_KEY,
            message: format!(
                "index `{index}` argument `{arg}` must be an orderable scalar type, \
                 but found `{resolved}`"
            ),
            span,
        }),
    }
}

/// Compile the members nested inside a group into nodes. Fields and groups
/// recurse; an index here is an error, since indexes are direct members of the
/// resource.
fn group_members(group: &GroupDecl, errors: &mut Vec<SchemaError>) -> Vec<Node> {
    let mut members = Vec::new();
    let mut names = Namespace::default();

    for member in &group.members {
        match member {
            ResourceMember::Index(index) => errors.push(SchemaError {
                code: SCHEMA_INDEX_IN_GROUP,
                message: format!(
                    "index `{}` cannot be declared inside group `{}`; \
                     declare indexes as direct resource members",
                    index.name, group.name
                ),
                span: index.span,
            }),
            _ => {
                names.check(member_name(member), member_span(member), errors);
                members.push(member_node(member, errors));
            }
        }
    }

    members
}

/// The name of a non-index resource member.
fn member_name(member: &ResourceMember) -> &str {
    match member {
        ResourceMember::Field(field) => &field.name,
        ResourceMember::Group(group) => &group.name,
        ResourceMember::Index(index) => &index.name,
    }
}

/// The span of a resource member.
fn member_span(member: &ResourceMember) -> SourceSpan {
    match member {
        ResourceMember::Field(field) => field.span,
        ResourceMember::Group(group) => group.span,
        ResourceMember::Index(index) => index.span,
    }
}

/// Compile one non-index resource member (a field or a group) into a [`Node`]:
/// an unkeyed plain field is a top-level `Slot`; a `sequence[T]` field and a
/// keyed field are both keyed-leaf `Slot`s; a group is a `Group` with recursed
/// members. An index is not a node and is handled by the caller.
fn member_node(member: &ResourceMember, errors: &mut Vec<SchemaError>) -> Node {
    match member {
        ResourceMember::Field(field) if field.keys.is_empty() => {
            // `name: sequence[T]` is sugar for the `name(pos: int): T` keyed leaf,
            // so it becomes a keyed `Slot` rather than a plain top-level field.
            match Type::resolve(&field.ty) {
                Type::Sequence(element) => sequence_leaf(field, *element),
                ty => slot_node(field, ty, vec![], field.required),
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
                stable_id: group.stable_id.clone(),
                key_params: group.keys.iter().map(key_def).collect(),
                members: group_members(group, errors),
                element: Element::Group,
            }
        }
        ResourceMember::Index(_) => unreachable!("indexes are not compiled to nodes"),
    }
}

/// A `Slot` node for `field`, carrying its value type, key parameters (empty for
/// a plain field, the keyed-leaf keys otherwise), and required flag.
fn slot_node(field: &FieldDecl, ty: Type, key_params: Vec<KeyDef>, required: bool) -> Node {
    Node {
        name: field.name.clone(),
        docs: field.docs.clone(),
        stable_id: field.stable_id.clone(),
        key_params,
        members: Vec::new(),
        element: Element::Slot { ty, required },
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
        stable_id: index.stable_id.clone(),
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
