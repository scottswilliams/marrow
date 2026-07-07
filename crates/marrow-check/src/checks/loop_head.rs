//! The single semantic owner of "what does this loop head mean". One classifier,
//! [`classify_loop_iterable`], answers what an iterable in a `for` head is; the
//! binding frame ([`for_frame`]) and every head diagnostic ([`check_for_head`])
//! derive from that one classification, so scope reconstruction and the type pass
//! cannot drift. A single binding is key-first — it binds the outer key; additional
//! names bind the remaining key columns and the leaf value per the layer's arity.

use std::collections::HashMap;
use std::path::Path;

use marrow_codes::Code;
use marrow_schema::ScalarType;
use marrow_syntax::LoopOrder;

use crate::executable::SavedPlaceResolver;
use crate::infer::infer_type;
use crate::{
    BuiltinView, CHECK_COLLECTION_UNSUPPORTED, CheckDiagnostic, CheckedProgram, DiagnosticAnchor,
    DiagnosticPayload, MarrowType,
};

use super::diagnostics::key_type_diagnostic;
use super::ranges::range_endpoint_type;
use super::saved_paths::{
    checked_saved_expr, is_concrete_scalar_value, saved_path_value_type, saved_place_resource_type,
};

/// The one classification of a for-head iterable. Computed once per statement; the
/// binding frame and every head diagnostic derive from it.
pub(crate) enum LoopIterable {
    /// A range: exactly one name, its endpoint scalar. `reversed` is rejected.
    Range { endpoint: MarrowType },
    /// A saved layer: its remaining key columns outermost-first and the leaf value
    /// type. A store root is one column (the identity), so a composite identity
    /// never widens the arity. Legal arities: 1 (bind `key_columns[0]`) or
    /// `key_columns.len() + 1` (bind every column plus the leaf).
    SavedLayer {
        key_columns: Vec<MarrowType>,
        leaf: MarrowType,
    },
    /// A non-unique index branch: streams identities; the two-name value is the
    /// materialized record. Legal arities: 1 (identity) or 2 (identity + resource).
    IndexBranch {
        identity: MarrowType,
        resource: MarrowType,
    },
    /// A partial unique index branch: not fully addressed, so its key-count is
    /// wrong. Carries the shape for the key-count diagnostic.
    UniqueIndexPartial {
        name: String,
        key_count: usize,
        arg_count: usize,
    },
    /// A fully-keyed leaf, field, single-key entry, or whole record: one value, no
    /// key to stream.
    SingleSavedValue,
    /// A local keyed tree: its first key column and value. Arities 1 or 2.
    LocalTree { key: MarrowType, value: MarrowType },
    /// A local sequence: 1-based integer positions and elements. Arities 1 or 2.
    Sequence { element: MarrowType },
    /// A concrete non-iterable scalar; the type is the binding's recovery type.
    Scalar(MarrowType),
    /// Unresolved: defer, emitting nothing, so a cross-module value is not a false
    /// positive.
    Unknown,
}

/// The one owner of loop-head classification: `SavedPlaceResolver` and `infer_type`
/// are consulted for a loop head only here.
pub(crate) fn classify_loop_iterable(
    program: &CheckedProgram,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> LoopIterable {
    if let Some(endpoint) = range_endpoint_type(program, iterable, scope, aliases, file) {
        return LoopIterable::Range { endpoint };
    }
    if let Some(checked) = checked_saved_expr(program, iterable, scope, file)
        && checked.saved_place().is_some()
    {
        let resolver = SavedPlaceResolver::new(program);
        if let Some(info) = resolver.index_branch_info(&checked) {
            // A partial unique branch is not fully addressed; its key-count error owns
            // the rejection. Every other index branch — a fully-addressed unique lookup
            // or a non-unique branch — streams the store identity, so a single name
            // binds the identity and a two-name head adds the materialized record.
            if info.unique && info.arg_count != info.key_count {
                return LoopIterable::UniqueIndexPartial {
                    name: info.name.to_string(),
                    key_count: info.key_count,
                    arg_count: info.arg_count,
                };
            }
            let identity = resolver.key_type(&checked).unwrap_or(MarrowType::Unknown);
            let resource = checked
                .saved_place()
                .map(|place| saved_place_resource_type(program, place))
                .unwrap_or(MarrowType::Unknown);
            return LoopIterable::IndexBranch { identity, resource };
        }
        if resolver.addresses_single_value(&checked) {
            return LoopIterable::SingleSavedValue;
        }
        if let Some((key_columns, leaf)) = resolver.loop_layer_columns(&checked) {
            return LoopIterable::SavedLayer { key_columns, leaf };
        }
        if let Some(key) = resolver.key_type(&checked) {
            // A store root yields the record at each identity; a keyed child layer (for
            // example a range-bounded one, which `loop_layer_columns` leaves to this
            // fallback) yields its own leaf. `saved_path_value_type` resolves the right
            // one, so the store resource is not mistaken for a child leaf.
            let leaf = saved_path_value_type(program, iterable, scope, file);
            return LoopIterable::SavedLayer {
                key_columns: vec![key],
                leaf,
            };
        }
        // A saved place with neither a value nor a key here is a partial or
        // range-bounded shape whose arity/key-type diagnostic is owned by the
        // key-range and saved-key-argument checks; defer rather than pile on a
        // second not-iterable error at the same span.
        return LoopIterable::Unknown;
    }
    match infer_type(program, iterable, scope, aliases, file, &mut Vec::new()) {
        MarrowType::LocalTree { keys, value } => LoopIterable::LocalTree {
            key: keys.into_iter().next().unwrap_or(MarrowType::Unknown),
            value: *value,
        },
        MarrowType::Sequence(element) => LoopIterable::Sequence { element: *element },
        ty if is_concrete_scalar_value(iterable, &ty) => LoopIterable::Scalar(ty),
        _ => LoopIterable::Unknown,
    }
}

/// Whether `expr` is a recognized iterable collection — a saved layer/root, index
/// branch, local tree, or sequence — whose leaf scalar type must not be mistaken for
/// a non-iterable scalar by a `count`/`keys`/`values` argument rule.
pub(crate) fn is_recognized_collection(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> bool {
    matches!(
        classify_loop_iterable(program, expr, scope, aliases, file),
        LoopIterable::SavedLayer { .. }
            | LoopIterable::IndexBranch { .. }
            | LoopIterable::LocalTree { .. }
            | LoopIterable::Sequence { .. }
    )
}

/// The scope frame a `for` loop's body runs under: each bound name typed against the
/// iterable's classification. Shared by the type pass and cursor scope
/// reconstruction so the two cannot drift.
pub(crate) fn for_frame(
    program: &CheckedProgram,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> HashMap<String, MarrowType> {
    let iter = classify_loop_iterable(program, iterable, scope, aliases, file);
    let types = binding_types(&iter, binding.names.len());
    binding
        .names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.name.clone(),
                types.get(index).cloned().unwrap_or(MarrowType::Unknown),
            )
        })
        .collect()
}

/// The type each head name binds for a classification and name count. A valid arity
/// yields one type per name; an invalid arity still types the leading names to their
/// recovery types so a body use does not stack a second untyped-value cascade.
fn binding_types(iter: &LoopIterable, given: usize) -> Vec<MarrowType> {
    let int = || MarrowType::Primitive(ScalarType::Int);
    match iter {
        LoopIterable::Range { endpoint } => vec![endpoint.clone()],
        LoopIterable::Scalar(ty) => vec![ty.clone()],
        LoopIterable::SavedLayer { key_columns, leaf } => {
            if given == key_columns.len() + 1 {
                let mut types = key_columns.clone();
                types.push(leaf.clone());
                types
            } else {
                // 1 name binds the outer key; a wrong count still types the first name.
                key_columns.iter().take(1).cloned().collect()
            }
        }
        LoopIterable::IndexBranch { identity, resource } => {
            vec![identity.clone(), resource.clone()]
        }
        LoopIterable::LocalTree { key, value } => vec![key.clone(), value.clone()],
        LoopIterable::Sequence { element } => vec![int(), element.clone()],
        LoopIterable::UniqueIndexPartial { .. }
        | LoopIterable::SingleSavedValue
        | LoopIterable::Unknown => Vec::new(),
    }
    .into_iter()
    .take(given.max(1))
    .collect()
}

/// Every head diagnostic: the view-call ban, arity, `reversed` legality per class,
/// the not-iterable classes, and duplicate head names.
/// The read-only context a loop-head check consults: the program facts and the
/// name/alias scope the iterable resolves against. Bundled so the head check threads
/// one borrow rather than four positional arguments.
pub(crate) struct LoopHeadScope<'a> {
    pub program: &'a CheckedProgram,
    pub file: &'a Path,
    pub scope: &'a [HashMap<String, MarrowType>],
    pub aliases: &'a HashMap<String, Vec<String>>,
}

pub(crate) fn check_for_head(
    cx: &LoopHeadScope<'_>,
    binding: &marrow_syntax::ForBinding,
    order: LoopOrder,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let &LoopHeadScope {
        program,
        file,
        scope,
        aliases,
    } = cx;
    check_duplicate_head_names(file, binding, diagnostics);
    if let Some(builtin) = head_view_call(iterable) {
        diagnostics.push(CheckDiagnostic::new(
            Code::CheckLoopHeadViewCall,
            DiagnosticAnchor::at(file, iterable.span()),
            DiagnosticPayload::LoopHeadViewCall(builtin),
        ));
        return;
    }
    let iter = classify_loop_iterable(program, iterable, scope, aliases, file);
    let given = binding.names.len();
    let span = iterable.span();
    match iter {
        LoopIterable::Range { .. } => {
            if order == LoopOrder::Reversed {
                diagnostics.push(unsupported(
                    file,
                    span,
                    "spell a descending range with its endpoints and `by`, not `reversed`",
                ));
            }
            check_arity(file, span, given, 1, diagnostics);
        }
        LoopIterable::SavedLayer { key_columns, .. } => {
            let columns = key_columns.len();
            if given != 1 && given != columns + 1 {
                diagnostics.push(arity_error(file, span, columns, given));
            }
        }
        LoopIterable::IndexBranch { .. }
        | LoopIterable::LocalTree { .. }
        | LoopIterable::Sequence { .. } => {
            if given != 1 && given != 2 {
                diagnostics.push(arity_error(file, span, 1, given));
            }
        }
        LoopIterable::UniqueIndexPartial {
            name,
            key_count,
            arg_count,
        } => {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "unique index `{name}` expects {key_count} key argument(s), but {arg_count} were given",
                ),
            ));
        }
        LoopIterable::SingleSavedValue => {
            diagnostics.push(unsupported(
                file,
                span,
                "this saved path names a single value, which cannot be iterated",
            ));
        }
        LoopIterable::Scalar(_) => {
            diagnostics.push(unsupported(
                file,
                span,
                "this value is a scalar, which cannot be iterated",
            ));
        }
        LoopIterable::Unknown => {}
    }
}

fn check_arity(
    file: &Path,
    span: marrow_syntax::SourceSpan,
    given: usize,
    column_count: usize,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if given != column_count {
        diagnostics.push(arity_error(file, span, column_count, given));
    }
}

fn arity_error(
    file: &Path,
    span: marrow_syntax::SourceSpan,
    column_count: usize,
    given: usize,
) -> CheckDiagnostic {
    CheckDiagnostic::new(
        Code::CheckLoopHeadArity,
        DiagnosticAnchor::at(file, span),
        DiagnosticPayload::LoopHeadArity {
            column_count,
            given,
        },
    )
}

fn unsupported(file: &Path, span: marrow_syntax::SourceSpan, message: &str) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_COLLECTION_UNSUPPORTED, file, span, message)
}

/// A duplicate name in one head is a same-scope redeclaration; the head binding list
/// is one scope, so the second use is anchored at its own span.
fn check_duplicate_head_names(
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mut seen: HashMap<&str, marrow_syntax::SourceSpan> = HashMap::new();
    for name in &binding.names {
        if let Some(first) = seen.get(name.name.as_str()) {
            diagnostics.push(crate::driver::duplicate_declaration_diagnostic(
                file, &name.name, name.span, *first,
            ));
        } else {
            seen.insert(name.name.as_str(), name.span);
        }
    }
}

/// The builtin a head iterable directly names as a view call — `keys(x)` or
/// `values(x)` — or `None`. Only a direct single-segment call to the builtin
/// counts; a value bound to a name first is an ordinary sequence.
fn head_view_call(iterable: &marrow_syntax::Expression) -> Option<BuiltinView> {
    let marrow_syntax::Expression::Call { callee, args, .. } = iterable else {
        return None;
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if args.len() != 1 {
        return None;
    }
    match segments.as_slice() {
        [name] if name == "keys" => Some(BuiltinView::Keys),
        [name] if name == "values" => Some(BuiltinView::Values),
        _ => None,
    }
}
