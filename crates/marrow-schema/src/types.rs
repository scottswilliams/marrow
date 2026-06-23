//! The typed tree shapes a declaration compiles into: [`Type`] resolution and
//! the [`ResourceSchema`], [`StoreSchema`], and [`Node`] tree these crates
//! pattern-match on instead of re-reading source spellings.

use std::fmt;

use marrow_syntax::TypeRef;

use crate::ScalarType;

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
    /// Any bare or qualified name not decidable from the text alone: a resource
    /// or enum reference the checker confirms, or a typo.
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

    pub(crate) fn resolve_text(text: &str) -> Self {
        // `sequence[T]` is built-in element-type sugar; recurse on the element.
        if let Some(element) = crate::compile::sequence_element(text) {
            return Self::Sequence(Box::new(Self::resolve_text(element)));
        }
        if let Some(scalar) = scalar_type_from_name(text) {
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
    /// or unknown type. Two saved-data readers ask this one structural question —
    /// which scalar, if any, this type is: a saved key projects its orderable key
    /// scalar, and the runtime decodes a saved leaf by it. A named type still
    /// needs project-level resolution before its durable value meaning is known.
    pub fn scalar(&self) -> Option<ScalarType> {
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

/// Resolve a source surface spelling to the storable [`ScalarType`] it names, or
/// `None` for a name that is not a scalar spelling. This is the language/schema
/// inverse of [`ScalarType::name`]: it accepts the canonical spellings plus the
/// `string` and `ErrorCode` aliases, which both store as [`ScalarType::Str`]. The
/// store owns only the canonical [`ScalarType::name`]; spelling resolution is a
/// language concern and lives here.
pub fn scalar_type_from_name(name: &str) -> Option<ScalarType> {
    Some(match name {
        "bool" => ScalarType::Bool,
        "int" => ScalarType::Int,
        "string" | "ErrorCode" => ScalarType::Str,
        "bytes" => ScalarType::Bytes,
        "date" => ScalarType::Date,
        "instant" => ScalarType::Instant,
        "duration" => ScalarType::Duration,
        "decimal" => ScalarType::Decimal,
        _ => return None,
    })
}

/// Whether a source type spelling names `ErrorCode`. `ErrorCode` resolves to a
/// [`ScalarType::Str`] like `string`, so the spelling is otherwise lost; a field
/// or binding declared with it enforces the dotted-lowercase grammar on the
/// values it accepts. The one place that recognizes the spelling.
pub fn is_error_code_spelling(text: &str) -> bool {
    text.trim() == "ErrorCode"
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
    /// The schema node a saved-path chain (outermost first) names: every name but the
    /// last is a group layer to descend into, and the last names a member of that
    /// layer's tree, of any kind. `None` for an empty chain or a name the schema does
    /// not declare at that position. This is the one canonical resource-tree member
    /// walk; the typed accessors below project it to the shape each caller needs.
    pub fn node_at(&self, chain: &[&str]) -> Option<&Node> {
        let (&member, parents) = chain.split_last()?;
        let members = match parents {
            [] => &self.members,
            _ => &self.descend_layers(parents)?.members,
        };
        members.iter().find(|node| node.name == member)
    }

    /// The declared type of a stored field named by its saved-path chain
    /// (outermost first), where the last name is a scalar field and every earlier
    /// name is a group layer to descend into. `None` for an empty chain or a name
    /// the schema does not declare as that shape.
    ///
    /// A keyed-leaf layer read at the same position is [`Self::leaf_type`]; the two
    /// differ only in whether the terminal name is a field (a [`NodeKind::Slot`]) or
    /// a group (a [`NodeKind::Group`]) to descend.
    pub fn field_type(&self, chain: &[&str]) -> Option<&Type> {
        self.node_at(chain)?.plain_field_type()
    }

    /// The declared leaf value type of the keyed-leaf layer named by its chain
    /// (outermost first). `None` for an empty chain, an unknown layer, or a group
    /// layer, which has members rather than a leaf value.
    pub fn leaf_type(&self, layers: &[&str]) -> Option<&Type> {
        match &self.descend_layers(layers)?.kind {
            NodeKind::Slot { ty, .. } => Some(ty),
            NodeKind::Group => None,
        }
    }

    /// The innermost node of a non-empty chain of group layer names. `None` for an
    /// empty chain or an unknown name. A plain field (a `Slot` with no key
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

/// The layer node named `name`: a group or a keyed leaf — anything but a plain
/// field (a `Slot` with no key parameters), which is not a layer to descend.
fn layer_member<'a>(members: &'a [Node], name: &str) -> Option<&'a Node> {
    members
        .iter()
        .find(|node| node.name == name && !node.is_plain_field())
}

impl Node {
    /// Whether this node is a plain field: a `Slot` carrying no key parameters. A
    /// keyed leaf (`Slot` with key parameters) and a group are layers, not plain
    /// fields.
    pub fn is_plain_field(&self) -> bool {
        self.key_params.is_empty() && matches!(self.kind, NodeKind::Slot { .. })
    }

    /// Whether this node is a `required` plain field. Only a top-level or group field
    /// carries `required`; a keyed leaf and a group never do. A required field added over
    /// existing saved data needs an explicit data-evolution apply, since it cannot read as
    /// absent the way a sparse field can.
    pub fn is_required_field(&self) -> bool {
        self.key_params.is_empty() && matches!(self.kind, NodeKind::Slot { required: true, .. })
    }

    /// The type of this node when it is a plain field, else `None`.
    pub fn plain_field_type(&self) -> Option<&Type> {
        match &self.kind {
            NodeKind::Slot { ty, .. } if self.key_params.is_empty() => Some(ty),
            _ => None,
        }
    }

    /// Whether this node is a `Slot` declared `ErrorCode`, so a value written to
    /// it must satisfy the dotted-lowercase error-code grammar.
    pub fn is_error_code(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::Slot {
                error_code: true,
                ..
            }
        )
    }

    /// The type a single value cell of this `Slot` holds — a plain field's own
    /// type or a keyed-leaf entry's value type; `None` for a group. Evolution
    /// records this as the member's identity-aware leaf token, so a value-type
    /// change on a keyed-leaf value is detected by referent identity exactly as it
    /// is for a plain field.
    pub fn leaf_value_type(&self) -> Option<&Type> {
        match &self.kind {
            NodeKind::Slot { ty, .. } => Some(ty),
            NodeKind::Group => None,
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

    /// Does this store root qualify for the default `nextId` allocation policy?
    /// Only a store with exactly one `int` identity key does; composite
    /// identities, non-integer identities, and keyless singletons are
    /// application-provided. This is the one contract both the checker (which
    /// types `nextId(^root)`) and the runtime write planner (which allocates the
    /// next id) gate on.
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
/// distinguished by its [`NodeKind`]. The recursive `members` are filled only for
/// a group; a keyed leaf carries `key_params` and an empty `members`; a
/// top-level field carries neither key params nor members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub docs: Vec<String>,
    /// Empty for a top-level field; non-empty for any keyed leaf or keyed group.
    pub key_params: Vec<KeyDef>,
    /// The declared resource entry type for an explicit typed keyed-field layer.
    pub entry_type: Option<Type>,
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
    /// `error_code` records that the field was declared `ErrorCode`: its value
    /// stores as a `Str` like any other, but a value reaching it must satisfy the
    /// dotted-lowercase grammar, so the spelling cannot be recovered from `ty`.
    Slot {
        ty: Type,
        required: bool,
        error_code: bool,
    },
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
