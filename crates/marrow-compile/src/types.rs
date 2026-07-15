//! The project named-type registry: transparent aliases and the record type.
//!
//! This is the single owner of what a source type name denotes. A transparent
//! `alias Name = Type` denotes exactly its expansion — it mints no identity and
//! no constructor — so every annotation classification calls [`TypeRegistry::expand`]
//! before reading the spelling. A nominal `type Name: int in lo..hi` mints a
//! distinct type: the registry owns its identity — name, inclusive interval, and
//! `supports` capability set — while the image records only its base scalar, so
//! the interval is carried by the guard instructions the compiler emits, not by
//! an image type table. Two product kinds lower into image [`RecordTypeDef`]s,
//! the single canonical product-leaf order owner: the optional `resource` (a record
//! with required and sparse fields — scalar or closed-enum, no groups and no keyed
//! children; only scalar fields when it backs a `store`) and any number of dense
//! `struct` value types (every field required, non-durable, constructible and read
//! by value). Value types are built declare-then-fill so a field may name any other
//! value type regardless of order; the sole nesting restriction is acyclicity.

use std::cell::RefCell;
use std::collections::BTreeMap;

use marrow_codes::Code;
use marrow_image::{
    CollectionTypeDef, EnumId, EnumTypeDef, FieldDef, ImageDraft, ImageType, RecordTypeDef, Scalar,
    TypeId, VariantDef,
};
use marrow_syntax::{
    AliasDecl, EnumDecl, EnumMember, Expression, LiteralKind, NominalDecl, ResourceDecl,
    ResourceMember, SourceSpan, StructDecl, TypeExpr, UnaryOp, range_expr,
};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

/// The identity of a nominal type in [`TypeRegistry`] order, carried by the
/// lowered type so classification never re-reads the source spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NominalId(pub(crate) u16);

/// A concrete bare (non-optional) value type used as a `Option`/`Result` type
/// argument. Monomorphization keys an instantiation on the exact argument types,
/// so `Option[int]` and `Option[string]` are distinct instantiations, and
/// `Option[Option[int]]` nests through the [`GArg::Enum`] case. A resource record
/// is not a value type, so it is not a representable argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GArg {
    Scalar(ScalarType),
    Nominal(NominalId),
    Struct(TypeId),
    Enum(EnumId),
    /// A finite collection value (`List[T]` / `Map[K, V]`) by its image COLLTYPES
    /// index. The element/key/value source types live in the registry's collection
    /// table (`CollSpec`), so a nested collection or a nominal element keeps its
    /// source identity even though the image erases a nominal element to `int`.
    Collection(u16),
}

impl GArg {
    /// The image type this argument monomorphizes to as an enum payload leaf or a
    /// record field. A nominal erases to its base `int` (its interval is carried by
    /// guards, not the image), matching how a nominal is recorded everywhere else.
    pub(crate) fn image(self) -> ImageType {
        match self {
            GArg::Scalar(scalar) => ImageType::scalar(scalar.image()),
            GArg::Nominal(_) => ImageType::scalar(Scalar::Int),
            GArg::Struct(ty) => ImageType::Record {
                idx: ty.index(),
                optional: false,
            },
            GArg::Enum(id) => ImageType::Enum {
                idx: id.index(),
                optional: false,
            },
            GArg::Collection(idx) => ImageType::Collection {
                idx,
                optional: false,
            },
        }
    }

    /// The scalar this argument denotes, if it is one. `None` for a nominal,
    /// struct, enum, or collection argument — the callers that need a durable scalar
    /// field use this to reject a non-scalar field.
    pub(crate) fn as_scalar(self) -> Option<ScalarType> {
        match self {
            GArg::Scalar(scalar) => Some(scalar),
            _ => None,
        }
    }

    /// Whether this argument is admitted as a `Map` key type: a scalar key type
    /// (`int`/`bool`/`string`/`bytes`) or a nominal int (which erases to `int`).
    /// `decimal`, structs, enums, and collections are rejected as keys, mirroring the
    /// durable-key scalar family.
    pub(crate) fn is_key_type(self) -> bool {
        match self {
            GArg::Scalar(scalar) => matches!(
                scalar,
                ScalarType::Int | ScalarType::Bool | ScalarType::Text | ScalarType::Bytes
            ),
            GArg::Nominal(_) => true,
            GArg::Struct(_) | GArg::Enum(_) | GArg::Collection(_) => false,
        }
    }
}

/// One concrete collection instantiation, keyed by the *source* element/key/value
/// types so `List[Age]` and `List[int]` stay distinct even though both erase to the
/// same image. The registry's collection table indexes these in the same order the
/// image COLLTYPES table records them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollSpec {
    List { elem: GArg },
    Map { key: GArg, value: GArg },
}

/// A built-in generic instantiation: which built-in type an image enum index came
/// from, and with which argument(s). Recovered by [`TypeRegistry::generic_inst`]
/// so `match`, construction, and spelling can treat an `Option`/`Result` value
/// like the built-in it is rather than a user `enum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericInst {
    Option(GArg),
    Result(GArg, GArg),
}

/// The `none`/`some` and `ok`/`err` variant indices, fixed for every `Option` and
/// `Result` instantiation so construction, `match`, and `try` agree on the tag.
pub(crate) const OPTION_NONE: u16 = 0;
pub(crate) const OPTION_SOME: u16 = 1;
pub(crate) const RESULT_OK: u16 = 0;
pub(crate) const RESULT_ERR: u16 = 1;

/// Whether `name` is a built-in generic type the user cannot redeclare.
pub(crate) fn is_reserved_type_name(name: &str) -> bool {
    matches!(name, "Option" | "Result" | "List" | "Map")
}

/// The lazily-monomorphized built-in generic instantiations. Interior-mutable so a
/// shared `&TypeRegistry` can mint a fresh instantiation into the image draft the
/// first time a concrete `Option`/`Result` is used, while every later use of the
/// same argument reuses the same image enum index.
#[derive(Default)]
struct Generics {
    options: Vec<(GArg, EnumId)>,
    results: Vec<(GArg, GArg, EnumId)>,
    by_id: Vec<(EnumId, GenericInst)>,
}

/// The closed capability set a nominal declaration's `supports` list unlocks.
/// Each flag independently admits operators over the nominal (see the lowerer's
/// operator mapping); construction and `.checked` need no capability.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SupportSet {
    pub(crate) add: bool,
    pub(crate) subtract: bool,
    pub(crate) step: bool,
    pub(crate) scale: bool,
}

/// One nominal type: a distinct int-based type whose every value lies in the
/// inclusive interval `[lo, hi]`.
pub(crate) struct NominalInfo {
    pub(crate) name: String,
    pub(crate) lo: i64,
    pub(crate) hi: i64,
    pub(crate) supports: SupportSet,
}

/// One resolved record field, in declaration order. The field type is a bare value
/// type. A resource field is a scalar (durable-storable) or a closed enum
/// (`Option`/`Result`/a user `enum`) local value; a struct field may additionally be
/// another struct or a nominal. Nesting is admitted behind the value-graph
/// acyclicity proof.
pub(crate) struct FieldInfo {
    pub(crate) name: String,
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// The project's single record type.
pub(crate) struct RecordInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl RecordInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// One dense product type: a `struct` whose every field is present inline. It
/// shares the image [`RecordTypeDef`] representation with the resource record —
/// the single canonical product-leaf order owner — but is a distinct value type:
/// non-durable, constructed and read by value, every field required. A struct is
/// admitted as a parameter and a return type (carried as an `ImageType::Record`).
pub(crate) struct StructInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl StructInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        field_index(&self.fields, name)
    }
}

/// One enum-variant payload leaf: a named scalar carried by that variant, in
/// declaration order. The name is used for named construction; the image records
/// only the scalar.
pub(crate) struct EnumPayloadInfo {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
}

/// One selectable enum variant: its member name and dense scalar payload.
pub(crate) struct VariantInfo {
    pub(crate) name: String,
    pub(crate) payload: Vec<EnumPayloadInfo>,
}

/// One closed flat enum value type. It lowers to an image [`EnumTypeDef`]; its
/// distinct nominal identity lives here. Hierarchical categories are deferred, so
/// every variant is a selectable leaf.
pub(crate) struct EnumInfo {
    pub(crate) enum_id: EnumId,
    pub(crate) name: String,
    pub(crate) variants: Vec<VariantInfo>,
}

impl EnumInfo {
    /// The index and info of the variant named `name` in declaration order.
    pub(crate) fn variant(&self, name: &str) -> Option<(u16, &VariantInfo)> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.name == name)
            .map(|(index, variant)| (index as u16, variant))
    }
}

/// The index and info of the field named `name` in declaration order, shared by
/// the resource record and the dense struct so field lookup has one owner.
fn field_index<'f>(fields: &'f [FieldInfo], name: &str) -> Option<(u16, &'f FieldInfo)> {
    fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name == name)
        .map(|(index, field)| (index as u16, field))
}

/// The project named-type registry: the transparent aliases, the nominal int
/// types, the dense struct value types, and zero or one durable record type.
#[derive(Default)]
pub(crate) struct TypeRegistry {
    /// `alias name -> alias-free expanded target`. Cyclic aliases are reported
    /// at build and never enter this map.
    aliases: BTreeMap<String, TypeExpr>,
    nominals: Vec<NominalInfo>,
    structs: Vec<StructInfo>,
    enums: Vec<EnumInfo>,
    record: Option<RecordInfo>,
    generics: RefCell<Generics>,
    /// The concrete collection instantiations minted so far, in image COLLTYPES
    /// order. Interior-mutable so a shared `&TypeRegistry` can mint one on first use
    /// of a concrete `List`/`Map`, deduping by source element/key/value types.
    collections: RefCell<Vec<CollSpec>>,
}

impl TypeRegistry {
    /// The image enum index of `Option[inner]`, minting it into `draft` on first
    /// use and reusing it thereafter. Variant `0` is `none` (no payload) and
    /// variant `1` is `some` (the argument as its single payload leaf).
    pub(crate) fn instantiate_option(&self, draft: &mut ImageDraft, inner: GArg) -> EnumId {
        if let Some((_, id)) = self
            .generics
            .borrow()
            .options
            .iter()
            .find(|(arg, _)| *arg == inner)
        {
            return *id;
        }
        let name = draft.intern_string("Option");
        let none = draft.intern_string("none");
        let some = draft.intern_string("some");
        let id = draft.add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: none,
                    category: false,
                    payload: Vec::new(),
                },
                VariantDef {
                    name: some,
                    category: false,
                    payload: vec![inner.image()],
                },
            ],
        });
        let mut generics = self.generics.borrow_mut();
        generics.options.push((inner, id));
        generics.by_id.push((id, GenericInst::Option(inner)));
        id
    }

    /// The image enum index of `Result[ok, err]`, minting it into `draft` on first
    /// use and reusing it thereafter. Variant `0` is `ok` and variant `1` is `err`,
    /// each carrying its argument as its single payload leaf.
    pub(crate) fn instantiate_result(&self, draft: &mut ImageDraft, ok: GArg, err: GArg) -> EnumId {
        if let Some((_, _, id)) = self
            .generics
            .borrow()
            .results
            .iter()
            .find(|(o, e, _)| *o == ok && *e == err)
        {
            return *id;
        }
        let name = draft.intern_string("Result");
        let ok_name = draft.intern_string("ok");
        let err_name = draft.intern_string("err");
        let id = draft.add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: ok_name,
                    category: false,
                    payload: vec![ok.image()],
                },
                VariantDef {
                    name: err_name,
                    category: false,
                    payload: vec![err.image()],
                },
            ],
        });
        let mut generics = self.generics.borrow_mut();
        generics.results.push((ok, err, id));
        generics.by_id.push((id, GenericInst::Result(ok, err)));
        id
    }

    /// Resolve a type annotation to a bare value type (a [`GArg`]), monomorphizing
    /// an `Option`/`Result` application into `draft` on first use. `None` for an
    /// optional, a sequence, the resource record, or a name not yet declared as a
    /// value type. Resource field-type resolution uses this to admit a scalar or a
    /// built-in `Option`/`Result` field; because the record builds before the enum
    /// table, a name resolving to a user `enum` is not yet visible here.
    pub(crate) fn resolve_garg(
        &self,
        draft: &mut ImageDraft,
        annotation: &TypeExpr,
    ) -> Option<GArg> {
        self.resolve_garg_expanded(draft, &self.expand(annotation))
    }

    fn resolve_garg_expanded(&self, draft: &mut ImageDraft, ty: &TypeExpr) -> Option<GArg> {
        match ty {
            TypeExpr::Name { text, .. } => {
                if let Some(scalar) = ScalarType::from_spelling(text) {
                    Some(GArg::Scalar(scalar))
                } else if let Some((id, _)) = self.nominal_by_name(text) {
                    Some(GArg::Nominal(id))
                } else if let Some(info) = self.struct_by_name(text) {
                    Some(GArg::Struct(info.type_id))
                } else {
                    self.enum_by_name(text).map(|info| GArg::Enum(info.enum_id))
                }
            }
            TypeExpr::Apply { head, args, .. } => match head.as_str() {
                "Option" => {
                    let [arg] = args.as_slice() else { return None };
                    let inner = self.resolve_garg(draft, arg)?;
                    Some(GArg::Enum(self.instantiate_option(draft, inner)))
                }
                "Result" => {
                    let [ok, err] = args.as_slice() else {
                        return None;
                    };
                    let ok = self.resolve_garg(draft, ok)?;
                    let err = self.resolve_garg(draft, err)?;
                    Some(GArg::Enum(self.instantiate_result(draft, ok, err)))
                }
                "List" => {
                    let [elem] = args.as_slice() else { return None };
                    let elem = self.resolve_garg(draft, elem)?;
                    Some(GArg::Collection(self.instantiate_list(draft, elem)))
                }
                "Map" => {
                    let [key, value] = args.as_slice() else {
                        return None;
                    };
                    let key = self.resolve_garg(draft, key)?;
                    // A map key is drawn from the durable-key scalar family; a struct,
                    // enum, collection, or decimal key is not admitted.
                    if !key.is_key_type() {
                        return None;
                    }
                    let value = self.resolve_garg(draft, value)?;
                    Some(GArg::Collection(self.instantiate_map(draft, key, value)))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// The image COLLTYPES index of `List[elem]`, minting it into `draft` on first
    /// use and reusing it thereafter. Dedup is by the *source* element type, so
    /// `List[Age]` and `List[int]` stay distinct rows even though both erase to
    /// `List[int]` in the image.
    pub(crate) fn instantiate_list(&self, draft: &mut ImageDraft, elem: GArg) -> u16 {
        let spec = CollSpec::List { elem };
        if let Some(index) = self.collections.borrow().iter().position(|s| *s == spec) {
            return index as u16;
        }
        let id = draft.add_collection_type(CollectionTypeDef::List { elem: elem.image() });
        let mut collections = self.collections.borrow_mut();
        debug_assert_eq!(id.index() as usize, collections.len());
        collections.push(spec);
        id.index()
    }

    /// The image COLLTYPES index of `Map[key, value]`, minting it on first use and
    /// reusing it thereafter, deduped by source key/value types.
    pub(crate) fn instantiate_map(&self, draft: &mut ImageDraft, key: GArg, value: GArg) -> u16 {
        let spec = CollSpec::Map { key, value };
        if let Some(index) = self.collections.borrow().iter().position(|s| *s == spec) {
            return index as u16;
        }
        let id = draft.add_collection_type(CollectionTypeDef::Map {
            key: key.image(),
            value: value.image(),
        });
        let mut collections = self.collections.borrow_mut();
        debug_assert_eq!(id.index() as usize, collections.len());
        collections.push(spec);
        id.index()
    }

    /// The source element/key/value spec of a minted collection instantiation.
    pub(crate) fn collection_spec(&self, idx: u16) -> CollSpec {
        self.collections.borrow()[idx as usize]
    }

    /// The source spelling of a collection instantiation (`List[T]` / `Map[K, V]`),
    /// used in diagnostics and cycle labels.
    pub(crate) fn collection_spelling(&self, idx: u16) -> String {
        match self.collection_spec(idx) {
            CollSpec::List { elem } => format!("List[{}]", garg_spelling(self, elem)),
            CollSpec::Map { key, value } => format!(
                "Map[{}, {}]",
                garg_spelling(self, key),
                garg_spelling(self, value)
            ),
        }
    }

    /// The built-in instantiation an image enum index came from, or `None` for a
    /// user-declared `enum`.
    pub(crate) fn generic_inst(&self, id: EnumId) -> Option<GenericInst> {
        self.generics
            .borrow()
            .by_id
            .iter()
            .find(|(eid, _)| *eid == id)
            .map(|(_, inst)| *inst)
    }

    pub(crate) fn by_name(&self, name: &str) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.name == name)
    }

    pub(crate) fn struct_by_name(&self, name: &str) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.name == name)
    }

    pub(crate) fn struct_by_type(&self, ty: TypeId) -> Option<&StructInfo> {
        self.structs.iter().find(|info| info.type_id == ty)
    }

    pub(crate) fn enum_by_name(&self, name: &str) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.name == name)
    }

    pub(crate) fn enum_by_id(&self, id: EnumId) -> Option<&EnumInfo> {
        self.enums.iter().find(|info| info.enum_id == id)
    }

    pub(crate) fn nominal_by_name(&self, name: &str) -> Option<(NominalId, &NominalInfo)> {
        self.nominals
            .iter()
            .position(|info| info.name == name)
            .map(|index| (NominalId(index as u16), &self.nominals[index]))
    }

    pub(crate) fn nominal(&self, id: NominalId) -> &NominalInfo {
        &self.nominals[id.0 as usize]
    }

    pub(crate) fn by_name_for_type(&self, ty: TypeId) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.type_id == ty)
    }

    /// The alias-free form of a type annotation: every name that is an alias is
    /// replaced by its expanded target, structurally, so classification reads
    /// only scalar spellings and declared type names. Diagnostics stay on the
    /// caller's annotation span — expansion carries the alias target's spans,
    /// which point at another declaration.
    pub(crate) fn expand(&self, ty: &TypeExpr) -> TypeExpr {
        match ty {
            TypeExpr::Name { text, .. } => match self.aliases.get(text) {
                Some(target) => target.clone(),
                None => ty.clone(),
            },
            TypeExpr::Optional { inner, span } => TypeExpr::Optional {
                inner: Box::new(self.expand(inner)),
                span: *span,
            },
            TypeExpr::Sequence { element, span } => TypeExpr::Sequence {
                element: Box::new(self.expand(element)),
                span: *span,
            },
            TypeExpr::Apply { head, args, span } => TypeExpr::Apply {
                head: head.clone(),
                args: args.iter().map(|arg| self.expand(arg)).collect(),
                span: *span,
            },
            TypeExpr::Identity(_) => ty.clone(),
        }
    }

    /// Every generic `Option`/`Result` instantiation minted so far, paired with its
    /// image enum index. The value-type cycle check reads these so a nesting routed
    /// through a built-in generic (`struct S` with a `some(S)` field) is caught.
    pub(crate) fn generic_instantiations(&self) -> Vec<(EnumId, GenericInst)> {
        self.generics
            .borrow()
            .by_id
            .iter()
            .map(|(id, inst)| (*id, *inst))
            .collect()
    }

    /// Build the registry: the alias table (duplicates, resource-name collisions,
    /// and cycles rejected; targets pre-expanded to alias-free form and validated
    /// against the known types), then the value types in two passes.
    ///
    /// Value types (the one resource record, the dense structs, and the closed
    /// enums) are built declare-then-fill: pass one reserves every type's image
    /// index with empty members and decides name conflicts, so pass two can resolve
    /// each field or payload against the full set of declared types regardless of
    /// declaration order — a struct field may name a later struct or enum, two
    /// structs may reference each other, and a resource field may name a user enum.
    /// The only nesting restriction is acyclicity: a value type may not contain
    /// itself directly or transitively, reported at check time (and independently
    /// re-rejected by the verifier). The resource record reserves its image index
    /// before the structs, so a project's durable root and sites keep the same
    /// record index whether or not dense structs are also declared.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        draft: &mut ImageDraft,
        aliases: &[(String, &AliasDecl)],
        nominals: &[(String, &NominalDecl)],
        structs: &[(String, &StructDecl)],
        enums: &[(String, &EnumDecl)],
        resources: &[(String, &ResourceDecl)],
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        let mut registry = Self {
            aliases: build_alias_table(aliases, resources, structs, enums, diagnostics),
            nominals: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            record: None,
            generics: RefCell::default(),
            collections: RefCell::default(),
        };
        registry.nominals =
            build_nominals(&registry, nominals, resources, structs, enums, diagnostics);

        // Pass one: reserve every value type's image index with empty members and
        // decide name conflicts. The record reserves first (image index 0).
        let record_decl = declare_record(draft, &mut registry, resources, diagnostics);
        let struct_decls = declare_structs(draft, &mut registry, structs, resources, diagnostics);
        let enum_decls = declare_enums(draft, &mut registry, enums, resources, diagnostics);

        // Pass two: resolve and fill each definition's members against the full
        // registry, monomorphizing any `Option`/`Result` field type on first use.
        fill_record(draft, &mut registry, record_decl.as_ref(), diagnostics);
        fill_structs(draft, &mut registry, &struct_decls, diagnostics);
        fill_enums(draft, &mut registry, &enum_decls, diagnostics);

        reject_value_cycles(&registry, structs, resources, diagnostics);
        validate_alias_targets(&registry, aliases, diagnostics);
        registry
    }
}

/// Resolve the alias declarations to an alias-free name → target map. A
/// duplicate alias name or a collision with a resource name is a
/// `check.name_conflict`; an alias on a cyclic chain is a `check.recursion`
/// and does not enter the map.
fn build_alias_table(
    aliases: &[(String, &AliasDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> BTreeMap<String, TypeExpr> {
    let mut raw: BTreeMap<String, TypeExpr> = BTreeMap::new();
    for (file, decl) in aliases {
        // A parse error blocks compilation before this runs, so a missing target
        // only means the declaration itself was reported; skip it quietly.
        let Some(ty) = &decl.ty else { continue };
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if raw.contains_key(&decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("an alias named `{}` is already declared", decl.name),
            ));
            continue;
        }
        if resources
            .iter()
            .any(|(_, resource)| resource.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a resource", decl.name),
            ));
            continue;
        }
        if structs.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a struct", decl.name),
            ));
            continue;
        }
        if enums.iter().any(|(_, item)| item.name == decl.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as an enum", decl.name),
            ));
            continue;
        }
        raw.insert(decl.name.clone(), ty.clone());
    }

    // Reject every alias that can reach itself through the names its target
    // mentions. Reported per alias on the cycle, at its declaration.
    let cyclic: Vec<String> = raw
        .keys()
        .filter(|name| reaches_itself(name, &raw))
        .cloned()
        .collect();
    for name in &cyclic {
        let (file, decl) = aliases
            .iter()
            .find(|(_, decl)| &decl.name == name)
            .expect("cyclic alias came from the declaration list");
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckRecursion.as_str(),
            file,
            decl.name_span,
            format!("alias `{name}` is part of a cyclic alias chain"),
        ));
        raw.remove(name);
    }

    // The survivors are acyclic; expand each target to alias-free form.
    let expanded: BTreeMap<String, TypeExpr> = raw
        .keys()
        .map(|name| (name.clone(), expand_in(&raw, &raw[name])))
        .collect();
    expanded
}

/// Whether alias `start` can reach itself by following alias-name references.
fn reaches_itself(start: &str, table: &BTreeMap<String, TypeExpr>) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    referenced_names(&table[start], &mut |name| {
        if table.contains_key(name) {
            stack.push(name);
        }
    });
    let mut visited: Vec<&str> = Vec::new();
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if visited.contains(&node) {
            continue;
        }
        visited.push(node);
        if let Some(target) = table.get(node) {
            referenced_names(target, &mut |name| {
                if table.contains_key(name) {
                    stack.push(name);
                }
            });
        }
    }
    false
}

/// Visit every type name a target mentions. `referenced_names` and `expand_in`
/// walk the same structure; keeping the traversal here keeps them aligned.
fn referenced_names<'t>(ty: &'t TypeExpr, visit: &mut impl FnMut(&'t str)) {
    match ty {
        TypeExpr::Name { text, .. } => visit(text),
        TypeExpr::Optional { inner, .. } => referenced_names(inner, visit),
        TypeExpr::Sequence { element, .. } => referenced_names(element, visit),
        TypeExpr::Apply { args, .. } => {
            for arg in args {
                referenced_names(arg, visit);
            }
        }
        TypeExpr::Identity(_) => {}
    }
}

/// Expand a target over an acyclic raw table (build-time twin of
/// [`TypeRegistry::expand`], which reads the finished alias-free map).
fn expand_in(table: &BTreeMap<String, TypeExpr>, ty: &TypeExpr) -> TypeExpr {
    match ty {
        TypeExpr::Name { text, .. } => match table.get(text) {
            Some(target) => expand_in(table, target),
            None => ty.clone(),
        },
        TypeExpr::Optional { inner, span } => TypeExpr::Optional {
            inner: Box::new(expand_in(table, inner)),
            span: *span,
        },
        TypeExpr::Sequence { element, span } => TypeExpr::Sequence {
            element: Box::new(expand_in(table, element)),
            span: *span,
        },
        TypeExpr::Apply { head, args, span } => TypeExpr::Apply {
            head: head.clone(),
            args: args.iter().map(|arg| expand_in(table, arg)).collect(),
            span: *span,
        },
        TypeExpr::Identity(_) => ty.clone(),
    }
}

/// Every alias must denote a known type, used or not: its expansion is a scalar,
/// the record type, or one optional over either. An unknown name is a
/// `check.type` at the alias; a well-formed but unadmitted shape is a
/// `check.unsupported`.
fn validate_alias_targets(
    registry: &TypeRegistry,
    aliases: &[(String, &AliasDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    for (file, decl) in aliases {
        let Some(expanded) = registry.aliases.get(&decl.name) else {
            continue; // duplicate or cyclic: already reported
        };
        let head = match expanded {
            TypeExpr::Optional { inner, .. } => inner.as_ref(),
            other => other,
        };
        match head {
            TypeExpr::Name { text, .. } => {
                if ScalarType::from_spelling(text).is_none()
                    && registry.by_name(text).is_none()
                    && registry.nominal_by_name(text).is_none()
                    && registry.struct_by_name(text).is_none()
                    && registry.enum_by_name(text).is_none()
                {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        decl.span,
                        format!("alias `{}` does not name a known type: `{text}`", decl.name),
                    ));
                }
            }
            _ => diagnostics.push(unsupported(
                file,
                decl.span,
                &format!("the target type of alias `{}`", decl.name),
            )),
        }
    }
}

/// Resolve the nominal type declarations against the aliases already installed
/// in `registry`. A name collision with an alias, resource, or earlier nominal
/// is a `check.name_conflict`; a base that does not expand to `int` is a
/// `check.unsupported`; a non-literal, stepped, or empty interval is a
/// `check.type`; the capability list must draw from the closed set without
/// repeats. A declaration with a defect is dropped whole rather than admitted
/// half-checked.
#[allow(clippy::too_many_arguments)]
fn build_nominals(
    registry: &TypeRegistry,
    nominals: &[(String, &NominalDecl)],
    resources: &[(String, &ResourceDecl)],
    structs: &[(String, &StructDecl)],
    enums: &[(String, &EnumDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<NominalInfo> {
    let mut built: Vec<NominalInfo> = Vec::new();
    for (file, decl) in nominals {
        // A parse error blocks compilation before this runs; a missing piece
        // only means the declaration itself was reported, so skip it quietly.
        let (Some(base), Some(interval)) = (&decl.base, &decl.interval) else {
            continue;
        };
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        // Scalar spellings are keywords the parser already rejects as names;
        // owning them here keeps the conflict predicate self-contained.
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || structs.iter().any(|(_, item)| item.name == decl.name)
            || enums.iter().any(|(_, item)| item.name == decl.name)
            || built.iter().any(|info| info.name == decl.name)
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        match scalar_of(&registry.expand(base)) {
            Some(ScalarType::Int) => {}
            Some(other) => {
                diagnostics.push(unsupported(
                    file,
                    base.span(),
                    &format!("a nominal type over `{}`", other.spelling()),
                ));
                continue;
            }
            None => {
                diagnostics.push(unsupported(file, base.span(), "this nominal base type"));
                continue;
            }
        }
        let Some((lo, hi)) = nominal_interval(file, interval, diagnostics) else {
            continue;
        };
        let Some(supports) = support_set(file, decl, diagnostics) else {
            continue;
        };
        built.push(NominalInfo {
            name: decl.name.clone(),
            lo,
            hi,
            supports,
        });
    }
    built
}

/// Evaluate a nominal `in` range to its inclusive `[lo, hi]` bounds. The range
/// follows the language's range operators — `lo..hi` excludes the end, `lo..=hi`
/// includes it — with int-literal bounds (a leading `-` allowed), no step, and
/// at least one admitted value.
fn nominal_interval(
    file: &str,
    interval: &Expression,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(i64, i64)> {
    let error = |diagnostics: &mut Vec<SourceDiagnostic>, span, message: &str| {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            span,
            message.to_string(),
        ));
        None
    };
    let Some(range) = range_expr(interval) else {
        return error(
            diagnostics,
            interval.span(),
            "a nominal interval is a range of int literals, such as `0..150`",
        );
    };
    if range.step.is_some() {
        return error(diagnostics, range.span, "a nominal interval takes no step");
    }
    let (Some(start), Some(end)) = (range.start, range.end) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval needs both bounds",
        );
    };
    let (Some(lo), Some(end_value)) = (literal_int(start), literal_int(end)) else {
        return error(
            diagnostics,
            range.span,
            "a nominal interval's bounds are int literals",
        );
    };
    // Normalize the end-exclusive spelling to the inclusive upper bound. A
    // literal never spells `i64::MIN`, so the exclusive form always has a
    // representable predecessor; the checked form keeps that self-evident.
    let hi = if range.inclusive_end {
        Some(end_value)
    } else {
        end_value.checked_sub(1)
    };
    match hi {
        Some(hi) if lo <= hi => Some((lo, hi)),
        _ => error(diagnostics, range.span, "this interval admits no values"),
    }
}

/// The value of an int literal, or a negated int literal, or `None`.
fn literal_int(expr: &Expression) -> Option<i64> {
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => crate::lower::parse_int(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match &**operand {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => crate::lower::parse_int(text).and_then(i64::checked_neg),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a declaration's `supports` spellings against the closed capability
/// set, rejecting an unknown or repeated capability.
fn support_set(
    file: &str,
    decl: &NominalDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<SupportSet> {
    let mut supports = SupportSet::default();
    for spelling in &decl.supports {
        let flag = match spelling.name.as_str() {
            "add" => &mut supports.add,
            "subtract" => &mut supports.subtract,
            "step" => &mut supports.step,
            "scale" => &mut supports.scale,
            other => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    spelling.span,
                    format!(
                        "unknown capability `{other}`; the capabilities are add, subtract, step, scale"
                    ),
                ));
                return None;
            }
        };
        if *flag {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                spelling.span,
                format!("capability `{}` is repeated", spelling.name),
            ));
            return None;
        }
        *flag = true;
    }
    Some(supports)
}

/// One struct reserved in pass one: the file it was declared in, its declaration,
/// and the image record index it will fill in pass two.
struct ReservedStruct<'a> {
    file: String,
    decl: &'a StructDecl,
    type_id: TypeId,
}

/// Pass one for the dense struct types: reserve each admitted struct's image
/// [`RecordTypeDef`] index (empty for now) and register its name, so pass two may
/// resolve a field that names any other struct or enum. A name collision with a
/// scalar, alias, nominal, resource, or earlier struct is a `check.name_conflict`;
/// a colliding or reserved-name struct is dropped and never reserved.
fn declare_structs<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    structs: &'a [(String, &StructDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ReservedStruct<'a>> {
    let mut reserved: Vec<ReservedStruct<'a>> = Vec::new();
    for (file, decl) in structs {
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || registry.struct_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&decl.name);
        let type_id = draft.add_record_type(RecordTypeDef {
            name: name_id,
            fields: Vec::new(),
        });
        registry.structs.push(StructInfo {
            type_id,
            name: decl.name.clone(),
            fields: Vec::new(),
        });
        reserved.push(ReservedStruct {
            file: file.clone(),
            decl,
            type_id,
        });
    }
    reserved
}

/// Pass two for the dense struct types: resolve each reserved struct's fields
/// against the full registry and fill both the registry info and the image record.
/// A struct field is the bare `name: Type` form over any value type — a scalar,
/// nominal, another struct, or a closed enum (`Option`/`Result`/a user `enum`);
/// a group, keyed field, the `required` keyword, an optional type, or an unknown
/// type is `check.unsupported`. A declaration with a member defect is dropped
/// whole (its reserved image record stays empty and its name leaves the registry)
/// so a later construction or match cannot resolve against a broken struct.
fn fill_structs(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedStruct<'_>],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut dropped: Vec<TypeId> = Vec::new();
    for item in reserved {
        match struct_fields(draft, registry, &item.file, item.decl, diagnostics) {
            Some((fields, field_defs)) => {
                draft.set_record_fields(item.type_id, field_defs);
                if let Some(info) = registry
                    .structs
                    .iter_mut()
                    .find(|info| info.type_id == item.type_id)
                {
                    info.fields = fields;
                }
            }
            None => dropped.push(item.type_id),
        }
    }
    registry
        .structs
        .retain(|info| !dropped.contains(&info.type_id));
}

/// Resolve a struct's members to its required value fields and their image
/// definitions, or `None` if any member is not the bare `name: Type` form over a
/// value type.
fn struct_fields(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &StructDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<FieldInfo>, Vec<FieldDef>)> {
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a struct group"));
            ok = false;
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed struct field"));
            ok = false;
            continue;
        }
        if field.required {
            diagnostics.push(unsupported(
                file,
                field.span,
                "the `required` keyword on a struct field (struct fields are always required)",
            ));
            ok = false;
            continue;
        }
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional struct field type",
            ));
            ok = false;
            continue;
        }
        let Some(field_ty) = registry.resolve_garg(draft, &field.ty) else {
            diagnostics.push(unsupported(file, field.ty.span(), "this struct field type"));
            ok = false;
            continue;
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: field_ty.image(),
            required: true,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            ty: field_ty,
            required: true,
        });
    }
    ok.then_some((fields, field_defs))
}

/// One enum reserved in pass one: the file it was declared in, its declaration,
/// and the image ENUMS index it will fill in pass two.
struct ReservedEnum<'a> {
    file: String,
    decl: &'a EnumDecl,
    enum_id: EnumId,
}

/// Pass one for the closed flat enum types: reserve each admitted enum's image
/// [`EnumTypeDef`] index (empty for now) and register its name. A name collision
/// with a scalar, alias, nominal, resource, struct, or earlier enum is a
/// `check.name_conflict`; a colliding or reserved-name enum is dropped and never
/// reserved. Reserving user enums before pass two resolves any field types keeps
/// their image indices ahead of the `Option`/`Result` instantiations minted lazily
/// during field resolution.
fn declare_enums<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    enums: &'a [(String, &EnumDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<ReservedEnum<'a>> {
    let mut reserved: Vec<ReservedEnum<'a>> = Vec::new();
    for (file, decl) in enums {
        if is_reserved_type_name(&decl.name) {
            diagnostics.push(reserved_name(file, decl.name_span, &decl.name));
            continue;
        }
        if ScalarType::from_spelling(&decl.name).is_some()
            || registry.aliases.contains_key(&decl.name)
            || registry.nominal_by_name(&decl.name).is_some()
            || registry.struct_by_name(&decl.name).is_some()
            || resources
                .iter()
                .any(|(_, resource)| resource.name == decl.name)
            || registry.enum_by_name(&decl.name).is_some()
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                decl.name_span,
                format!("`{}` is already declared as a type", decl.name),
            ));
            continue;
        }
        let name_id = draft.intern_string(&decl.name);
        let enum_id = draft.add_enum_type(EnumTypeDef {
            name: name_id,
            variants: Vec::new(),
        });
        registry.enums.push(EnumInfo {
            enum_id,
            name: decl.name.clone(),
            variants: Vec::new(),
        });
        reserved.push(ReservedEnum {
            file: file.clone(),
            decl,
            enum_id,
        });
    }
    reserved
}

/// Pass two for the closed flat enum types: resolve each reserved enum's variants
/// and fill both the registry info and the image ENUMS entry. Hierarchy is
/// deferred: a `category` member or a member with nested members is
/// `check.unsupported`. A member's payload is the dense `name: Type` form over bare
/// scalars; an optional or non-scalar payload type is `check.unsupported`. A
/// declaration with a defect is dropped whole (its reserved image entry stays empty
/// and its name leaves the registry) so a later match cannot resolve against a
/// broken enum.
fn fill_enums(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    reserved: &[ReservedEnum<'_>],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let mut dropped: Vec<EnumId> = Vec::new();
    for item in reserved {
        match enum_variants(draft, registry, &item.file, item.decl, diagnostics) {
            Some((variants, variant_defs)) => {
                draft.set_enum_variants(item.enum_id, variant_defs);
                if let Some(info) = registry
                    .enums
                    .iter_mut()
                    .find(|info| info.enum_id == item.enum_id)
                {
                    info.variants = variants;
                }
            }
            None => dropped.push(item.enum_id),
        }
    }
    registry
        .enums
        .retain(|info| !dropped.contains(&info.enum_id));
}

/// Resolve an enum's members to its selectable variants and their image
/// definitions, or `None` if any member is unsupported. On the flat line every
/// member is a leaf: a `category` member or one with nested members is deferred.
fn enum_variants(
    draft: &mut ImageDraft,
    registry: &TypeRegistry,
    file: &str,
    decl: &EnumDecl,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<VariantInfo>, Vec<VariantDef>)> {
    let mut variants = Vec::new();
    let mut variant_defs = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut ok = true;
    for member in &decl.members {
        if member.category {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a `category` enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if !member.members.is_empty() {
            diagnostics.push(unsupported(
                file,
                member.span,
                "a nested enum member (hierarchical enums are deferred)",
            ));
            ok = false;
            continue;
        }
        if seen.contains(&member.name) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckNameConflict.as_str(),
                file,
                member.name_span,
                format!("enum member `{}` is already declared", member.name),
            ));
            ok = false;
            continue;
        }
        seen.push(member.name.clone());
        let Some((payload, payload_scalars)) = enum_payload(registry, file, member, diagnostics)
        else {
            ok = false;
            continue;
        };
        let name_id = draft.intern_string(&member.name);
        variant_defs.push(VariantDef {
            name: name_id,
            category: false,
            payload: payload_scalars
                .iter()
                .map(|scalar| ImageType::scalar(scalar.image()))
                .collect(),
        });
        variants.push(VariantInfo {
            name: member.name.clone(),
            payload,
        });
    }
    ok.then_some((variants, variant_defs))
}

/// Resolve one member's payload fields to their scalars and info, or `None` when
/// a field is not the bare `name: scalar` form.
fn enum_payload(
    registry: &TypeRegistry,
    file: &str,
    member: &EnumMember,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(Vec<EnumPayloadInfo>, Vec<ScalarType>)> {
    let mut payload = Vec::new();
    let mut scalars = Vec::new();
    let mut ok = true;
    for field in &member.payload {
        if matches!(registry.expand(&field.ty), TypeExpr::Optional { .. }) {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "an optional enum payload field type",
            ));
            ok = false;
            continue;
        }
        let Some(scalar) = scalar_of(&registry.expand(&field.ty)) else {
            diagnostics.push(unsupported(
                file,
                field.ty.span(),
                "this enum payload field type",
            ));
            ok = false;
            continue;
        };
        payload.push(EnumPayloadInfo {
            name: field.name.clone(),
            scalar,
        });
        scalars.push(scalar);
    }
    ok.then_some((payload, scalars))
}

/// Pass one for the one admitted record type: reserve its image [`RecordTypeDef`]
/// index (empty for now, ahead of the structs) and register its name, returning the
/// surviving resource declaration for pass two. More than one resource, or a
/// reserved resource name, drops the record.
fn declare_record<'a>(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    resources: &'a [(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<(String, &'a ResourceDecl)> {
    if resources.len() > 1 {
        for (file, resource) in &resources[1..] {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                resource.name_span,
                "the beta line admits only one resource type per project".to_string(),
            ));
        }
    }
    let (file, resource) = resources.first()?;
    if is_reserved_type_name(&resource.name) {
        diagnostics.push(reserved_name(file, resource.name_span, &resource.name));
        return None;
    }
    let name_id = draft.intern_string(&resource.name);
    let type_id = draft.add_record_type(RecordTypeDef {
        name: name_id,
        fields: Vec::new(),
    });
    registry.record = Some(RecordInfo {
        type_id,
        name: resource.name.clone(),
        fields: Vec::new(),
    });
    Some((file.clone(), resource))
}

/// Pass two for the record type: resolve each field against the full registry and
/// fill both the registry info and the image record. A resource field is a scalar
/// (durable-storable) or a closed enum value (`Option`/`Result`/a user `enum`) held
/// locally; a nominal, struct, or unknown spelling is not an admitted field type. A
/// group or keyed member, and a bad field type, are `check.unsupported` and skip
/// only that member (the record keeps its other fields).
fn fill_record(
    draft: &mut ImageDraft,
    registry: &mut TypeRegistry,
    record_decl: Option<&(String, &ResourceDecl)>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let Some((file, resource)) = record_decl else {
        return;
    };
    let type_id = registry
        .record
        .as_ref()
        .expect("a reserved record is present when its declaration survived")
        .type_id;
    let mut fields = Vec::new();
    let mut field_defs = Vec::new();
    for member in &resource.members {
        let ResourceMember::Field(field) = member else {
            diagnostics.push(unsupported(file, member.span(), "a resource group"));
            continue;
        };
        if !field.keys.is_empty() {
            diagnostics.push(unsupported(file, field.span, "a keyed field"));
            continue;
        }
        let field_ty = match registry.resolve_garg(draft, &field.ty) {
            Some(ty @ (GArg::Scalar(_) | GArg::Enum(_))) => ty,
            _ => {
                diagnostics.push(unsupported(file, field.ty.span(), "this field type"));
                continue;
            }
        };
        let field_name_id = draft.intern_string(&field.name);
        field_defs.push(FieldDef {
            name: field_name_id,
            ty: field_ty.image(),
            required: field.required,
        });
        fields.push(FieldInfo {
            name: field.name.clone(),
            ty: field_ty,
            required: field.required,
        });
    }
    draft.set_record_fields(type_id, field_defs);
    if let Some(info) = registry.record.as_mut() {
        info.fields = fields;
    }
}

/// Reject a cycle in the value-containment graph at check time: a struct, record,
/// or enum that (directly or transitively) contains itself would be an infinite
/// value. Edges run from a product's fields and an enum's payload leaves to the
/// value types they name, including through the built-in `Option`/`Result`
/// instantiations minted during field resolution. Every struct or record on a cycle
/// is reported at its declaration with the cycle path; the verifier independently
/// re-rejects any cycle that still reaches it, so this is a source-facing check, not
/// the trust boundary.
fn reject_value_cycles(
    registry: &TypeRegistry,
    structs: &[(String, &StructDecl)],
    resources: &[(String, &ResourceDecl)],
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let graph = ValueGraph::build(registry);
    for info in &registry.structs {
        if let Some(path) = graph.cycle_through(ValueNode::Record(info.type_id)) {
            let (file, span) = structs
                .iter()
                .find(|(_, decl)| decl.name == info.name)
                .map(|(file, decl)| (file.clone(), decl.name_span))
                .expect("a reserved struct has a surviving declaration");
            diagnostics.push(value_cycle_diagnostic(&file, span, &info.name, &path));
        }
    }
    if let Some(record) = registry.record.as_ref()
        && let Some(path) = graph.cycle_through(ValueNode::Record(record.type_id))
    {
        let (file, span) = resources
            .first()
            .map(|(file, decl)| (file.clone(), decl.name_span))
            .expect("a reserved record has a surviving declaration");
        diagnostics.push(value_cycle_diagnostic(&file, span, &record.name, &path));
    }
}

fn value_cycle_diagnostic(
    file: &str,
    span: SourceSpan,
    name: &str,
    path: &[String],
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckRecursion.as_str(),
        file,
        span,
        format!(
            "value type `{name}` contains itself through the cycle {}",
            path.join(" -> ")
        ),
    )
}

/// A node in the value-containment graph: a record type (the resource record or a
/// struct — both are image records) or an enum type (a user enum or a built-in
/// `Option`/`Result` instantiation).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueNode {
    Record(TypeId),
    Enum(EnumId),
}

/// The value-containment graph over the project's records and enums, used to prove
/// acyclicity at check time.
struct ValueGraph {
    nodes: Vec<ValueNode>,
    labels: Vec<String>,
    edges: Vec<Vec<usize>>,
}

impl ValueGraph {
    fn build(registry: &TypeRegistry) -> Self {
        let mut nodes: Vec<ValueNode> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        let mut push = |node: ValueNode, label: String| {
            nodes.push(node);
            labels.push(label);
        };
        if let Some(record) = registry.record.as_ref() {
            push(ValueNode::Record(record.type_id), record.name.clone());
        }
        for info in &registry.structs {
            push(ValueNode::Record(info.type_id), info.name.clone());
        }
        for info in &registry.enums {
            push(ValueNode::Enum(info.enum_id), info.name.clone());
        }
        for (id, inst) in registry.generic_instantiations() {
            let label = garg_arg_to_lty_spelling(registry, inst);
            push(ValueNode::Enum(id), label);
        }
        let index_of = |target: ValueNode| nodes.iter().position(|node| *node == target);
        let arg_target = |arg: GArg| match arg {
            GArg::Struct(ty) => Some(ValueNode::Record(ty)),
            GArg::Enum(id) => Some(ValueNode::Enum(id)),
            // A collection is a finite value (an empty list/map terminates), so a
            // field reached only through one is not an infinite value: it adds no
            // containment edge, and `struct Node { kids: List[Node] }` is admitted.
            GArg::Scalar(_) | GArg::Nominal(_) | GArg::Collection(_) => None,
        };
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for (from, node) in nodes.iter().enumerate() {
            let targets: Vec<GArg> = match node {
                ValueNode::Record(ty) => registry
                    .record
                    .as_ref()
                    .filter(|record| record.type_id == *ty)
                    .map(|record| record.fields.iter().map(|field| field.ty).collect())
                    .or_else(|| {
                        registry
                            .struct_by_type(*ty)
                            .map(|info| info.fields.iter().map(|field| field.ty).collect())
                    })
                    .unwrap_or_default(),
                ValueNode::Enum(id) => match registry.generic_inst(*id) {
                    Some(GenericInst::Option(arg)) => vec![arg],
                    Some(GenericInst::Result(ok, err)) => vec![ok, err],
                    // A user enum's payload leaves are bare scalars, so it has no
                    // outgoing value-containment edges.
                    None => Vec::new(),
                },
            };
            for arg in targets {
                if let Some(target) = arg_target(arg)
                    && let Some(to) = index_of(target)
                {
                    edges[from].push(to);
                }
            }
        }
        ValueGraph {
            nodes,
            labels,
            edges,
        }
    }

    /// The label path of a cycle that passes through `node`, or `None` if `node` is
    /// not on any cycle. The path starts and ends at `node`'s label.
    fn cycle_through(&self, node: ValueNode) -> Option<Vec<String>> {
        let start = self.nodes.iter().position(|n| *n == node)?;
        let mut trail: Vec<usize> = Vec::new();
        let mut visited = vec![false; self.nodes.len()];
        if self.reaches(start, start, &mut visited, &mut trail) {
            let mut path: Vec<String> = trail.iter().map(|i| self.labels[*i].clone()).collect();
            path.push(self.labels[start].clone());
            return Some(path);
        }
        None
    }

    /// Whether `current` can reach `target` following one or more edges, recording
    /// the traversal in `trail`. A DFS bounded by `visited`; the graph is finite.
    fn reaches(
        &self,
        current: usize,
        target: usize,
        visited: &mut [bool],
        trail: &mut Vec<usize>,
    ) -> bool {
        trail.push(current);
        for &next in &self.edges[current] {
            if next == target {
                return true;
            }
            if !visited[next] {
                visited[next] = true;
                if self.reaches(next, target, visited, trail) {
                    return true;
                }
            }
        }
        trail.pop();
        false
    }
}

/// The source-facing spelling of the value type a built-in instantiation denotes,
/// used to label an `Option`/`Result` node in a reported value-type cycle.
fn garg_arg_to_lty_spelling(registry: &TypeRegistry, inst: GenericInst) -> String {
    match inst {
        GenericInst::Option(arg) => format!("Option[{}]", garg_spelling(registry, arg)),
        GenericInst::Result(ok, err) => format!(
            "Result[{}, {}]",
            garg_spelling(registry, ok),
            garg_spelling(registry, err)
        ),
    }
}

/// The source spelling of a bare value-type argument for cycle labels.
fn garg_spelling(registry: &TypeRegistry, arg: GArg) -> String {
    match arg {
        GArg::Scalar(scalar) => scalar.spelling().to_string(),
        GArg::Nominal(id) => registry.nominal(id).name.clone(),
        GArg::Struct(ty) => registry
            .struct_by_type(ty)
            .map(|info| info.name.clone())
            .or_else(|| {
                registry
                    .by_name_for_type(ty)
                    .map(|record| record.name.clone())
            })
            .unwrap_or_else(|| "struct".to_string()),
        GArg::Enum(id) => match registry.generic_inst(id) {
            Some(inst) => garg_arg_to_lty_spelling(registry, inst),
            None => registry
                .enum_by_id(id)
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "enum".to_string()),
        },
        GArg::Collection(idx) => registry.collection_spelling(idx),
    }
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

/// The diagnostic for a declaration that reuses a built-in generic type name.
fn reserved_name(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{name}` is a built-in generic type and cannot be redeclared"),
    )
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
