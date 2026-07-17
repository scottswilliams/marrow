//! FLIP 2 (BS01): the T3 `[]`-for-keys surface and the T2 angle-bracket generic
//! rule. Keyed access moves to square brackets (`^books[id]`,
//! `^patients[pid].visits[vid]`), key declarations mirror access with `[name: Type]`
//! columns, and generics move to angles (`Result<T, E>`, `fn identity<T>`) under the
//! T2 disambiguation rule: expression `<`/`>` are always comparison, and the one
//! `>=` token-split closes a generic that glues onto an assignment. Written fresh
//! against the new grammar (the layout corpus is allowlisted until the converter
//! flip rewrites it).

use marrow_syntax::{
    Declaration, DiagnosticReason, ExpectedSyntax, Expression, ParseDiagnosticReason,
    ResourceMember, Statement, TypeExpr, parse_source,
};

fn clean(source: &str) {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
}

fn has_reason(source: &str, reason: ParseDiagnosticReason) -> bool {
    let target = DiagnosticReason::Parser(reason);
    parse_source(source)
        .diagnostics
        .iter()
        .any(|d| d.reason == target)
}

/// Parse `body` as the statements of `fn run(...) { … }`, asserting a clean parse,
/// and return the first statement.
fn first_statement(header: &str, body: &str) -> Statement {
    let source = format!("module app\n{header} {{\n{body}\n}}\n");
    let parsed = parse_source(&source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    parsed.file.function("run").expect("run").body.statements[0].clone()
}

// ---- T3: keyed access in every access position ----

#[test]
fn a_root_key_access_parses_to_a_keyed_node() {
    let Statement::Assign { target, .. } = first_statement("fn run()", "^books[id] = b") else {
        panic!("expected assignment");
    };
    let Expression::Keyed { base, keys, .. } = target else {
        panic!("expected a keyed access target: {target:#?}");
    };
    assert!(
        matches!(*base, Expression::SavedRoot { .. }),
        "base is ^books"
    );
    assert_eq!(keys.len(), 1, "one key column");
}

#[test]
fn a_composite_key_carries_each_column() {
    let Statement::Assign { target, .. } = first_statement("fn run()", "^grid[a, b] = v") else {
        panic!("expected assignment");
    };
    let Expression::Keyed { keys, .. } = target else {
        panic!("expected keyed");
    };
    assert_eq!(keys.len(), 2, "two positional key columns: {keys:#?}");
}

#[test]
fn a_branch_chain_nests_keyed_and_field_nodes() {
    // `^patients[pid].visits[vid].obs[oid]` — a keyed root, keyed branch, keyed
    // branch, alternating `Keyed`/`Field`.
    let Statement::Assign { target, .. } =
        first_statement("fn run()", "^patients[pid].visits[vid].obs[oid] = o")
    else {
        panic!("expected assignment");
    };
    // outermost is Keyed[oid] over Field(obs) over Keyed[vid] over Field(visits)
    // over Keyed[pid] over SavedRoot(patients).
    let Expression::Keyed { base, keys, .. } = &target else {
        panic!("expected outer keyed: {target:#?}");
    };
    assert_eq!(keys.len(), 1);
    let Expression::Field { base, name, .. } = &**base else {
        panic!("expected .obs field");
    };
    assert_eq!(name, "obs");
    assert!(matches!(&**base, Expression::Keyed { .. }), "visits keyed");
}

#[test]
fn a_place_relative_keyed_field_parses() {
    clean("module app\nfn run(visit: int) {\n    const o = visit.obs[oid]\n    return\n}\n");
}

#[test]
fn a_field_through_keys_parses() {
    clean("module app\nfn run() {\n    const t = ^books[id].notes[nid].text\n    return\n}\n");
}

#[test]
fn exists_and_delete_take_keyed_targets() {
    clean(
        "module app\nfn run() {\n    if exists(^books[id]) {\n        delete ^patients[pid].visits[vid]\n    }\n    return\n}\n",
    );
}

#[test]
fn the_parens_and_brackets_law_showcase_line_parses() {
    // Address on the left in brackets, construction on the right in parens.
    let Statement::Assign { target, value, .. } = first_statement(
        "fn run(kind: int)",
        "^patients[pid].visits[vid].obs[obsId] = Patient.visits.obs(kind: kind)",
    ) else {
        panic!("expected assignment");
    };
    assert!(
        matches!(target, Expression::Keyed { .. }),
        "left is a keyed address: {target:#?}"
    );
    assert!(
        matches!(value, Expression::Call { .. }),
        "right is a construction call: {value:#?}"
    );
}

#[test]
fn a_named_key_argument_is_a_parse_level_rejection() {
    assert!(
        has_reason(
            "module app\nfn run() {\n    ^books[id: 3] = b\n}\n",
            ParseDiagnosticReason::NamedKeyArgument,
        ),
        "`^books[id: 3]` rejects the named key"
    );
}

#[test]
fn an_empty_keyed_group_is_rejected() {
    let parsed = parse_source("module app\nfn run() {\n    ^books[] = b\n}\n");
    assert!(
        !parsed.diagnostics.is_empty(),
        "a keyed access selects at least one key column"
    );
}

#[test]
fn an_unclosed_keyed_group_recovers_at_the_close_bracket() {
    assert!(
        has_reason(
            "module app\nfn run() {\n    const x = ^books[id\n}\n",
            ParseDiagnosticReason::Expected(ExpectedSyntax::CloseBracket),
        ),
        "an unclosed key group reports the missing `]`"
    );
}

// ---- T3: declaration-mirrors-access ----

#[test]
fn a_keyed_store_root_declares_its_columns_in_brackets() {
    let parsed = parse_source(
        "module app\nresource Patient {\n    required name: string\n}\nstore ^patients[pid: int]: Patient\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let store = parsed.file.store("patients").expect("store");
    assert_eq!(store.root.keys.len(), 1, "one key column pid");
    assert_eq!(store.root.keys[0].name, "pid");
}

#[test]
fn a_composite_store_root_declares_each_column() {
    let parsed = parse_source(
        "module app\nresource Enrollment {\n    required grade: int\n}\nstore ^enrollments[student: string, course: string]: Enrollment\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let store = parsed.file.store("enrollments").expect("store");
    assert_eq!(store.root.keys.len(), 2, "two key columns");
}

#[test]
fn keyed_leaf_and_branch_members_declare_in_brackets() {
    let parsed = parse_source(
        "module app\nresource Book {\n    required title: string\n    tags[pos: int]: string\n    notes[noteId: string] {\n        required text: string\n        tags[tagId: int] {\n            required weight: int\n        }\n    }\n}\nstore ^books[id: int]: Book\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let book = parsed.file.resource("Book").expect("Book");
    let tags = book
        .members
        .iter()
        .find_map(|m| match m {
            ResourceMember::Field(f) if f.name == "tags" => Some(f),
            _ => None,
        })
        .expect("keyed leaf tags");
    assert_eq!(tags.keys.len(), 1, "tags keyed by pos");
    assert!(
        book.members.iter().any(
            |m| matches!(m, ResourceMember::Group(g) if g.name == "notes" && g.keys.len() == 1)
        ),
        "notes keyed branch: {book:#?}"
    );
}

#[test]
fn a_keyed_local_and_keyed_parameter_declare_in_brackets() {
    // Keyed local `var cells[row, col]` and keyed parameter `scores[player]`.
    clean(
        "module app\nfn run(scores[player: string]: int) {\n    var cells[row: int, col: int]: int\n    return\n}\n",
    );
}

#[test]
fn index_declarations_are_bracketed() {
    let parsed = parse_source(
        "module app\nresource Book {\n    required isbn: string\n    required shelf: int\n}\nstore ^books[id: int]: Book {\n    index byShelf[shelf, id]\n    index byIsbn[isbn] unique\n}\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let store = parsed.file.store("books").expect("store");
    assert_eq!(store.indexes.len(), 2, "two indexes: {store:#?}");
    assert_eq!(store.indexes[0].args, vec!["shelf", "id"]);
    assert!(store.indexes[1].unique, "byIsbn is unique");
}

#[test]
fn an_index_still_written_with_parens_is_rejected() {
    let parsed = parse_source(
        "module app\nresource Book {\n    required shelf: int\n}\nstore ^books[id: int]: Book {\n    index byShelf(shelf)\n}\n",
    );
    assert!(
        !parsed.diagnostics.is_empty(),
        "the old paren index spelling no longer parses"
    );
}

// ---- Multi-argument generics in every comma-delimited declaration position ----
// A `<A, B>` type carries an internal comma. Every splitter that separates a
// comma-delimited declaration list must track angle depth so the internal comma
// does not split the enclosing list. `split_top_level_commas` does; the
// parameter-group splitter must too.

#[test]
fn a_multi_argument_generic_parameter_is_one_parameter() {
    let source = "module app\nfn f(r: Map<int, string>, x: int): int {\n    return x\n}\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    let function = parsed.file.function("f").expect("f");
    let names: Vec<&str> = function.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["r", "x"],
        "two parameters, not split by the generic comma"
    );
    assert!(
        matches!(&function.params[0].ty, TypeExpr::Apply { head, args, .. } if head == "Map" && args.len() == 2),
        "r is Map<int, string>: {:#?}",
        function.params[0].ty
    );
}

#[test]
fn a_nested_multi_argument_generic_parameter_is_one_parameter() {
    let source =
        "module app\nfn f(r: Map<int, Map<string, List<int>>>, x: int): int {\n    return x\n}\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    let names: Vec<&str> = parsed
        .file
        .function("f")
        .expect("f")
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["r", "x"]);
}

#[test]
fn a_multi_line_parameter_wraps_inside_its_generic() {
    // The generic argument list spans two physical lines. Angle depth keeps the
    // wrap from ending the parameter, just as `(`/`[` depth does.
    let source = "module app\nfn f(\n    r: Map<int,\n           string>,\n    x: int\n): int {\n    return x\n}\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    let names: Vec<&str> = parsed
        .file
        .function("f")
        .expect("f")
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["r", "x"]);
}

#[test]
fn a_keyed_parameter_value_generic_does_not_split_the_list() {
    let source =
        "module app\nfn f(scores[player: string]: Map<int, string>, x: int) {\n    return\n}\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    let names: Vec<&str> = parsed
        .file
        .function("f")
        .expect("f")
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["scores", "x"]);
}

#[test]
fn a_multi_argument_generic_key_type_does_not_split_the_key_list() {
    // Key columns are comma-separated; a generic key type carries its own comma.
    clean("module app\nfn run(cache[k: Map<int, string>, region: int]: bool) {\n    return\n}\n");
}

#[test]
fn a_multi_argument_generic_enum_payload_field_does_not_split_the_payload() {
    clean("module app\nenum E {\n    pair(m: Map<int, string>, count: int)\n    none\n}\n");
}

#[test]
fn an_unterminated_generic_parameter_does_not_split_into_a_clean_extra_parameter() {
    // With the angle-tracking splitter, the comma inside an unterminated
    // `Map<int, …` stays inside the parameter: the list is one (malformed)
    // parameter, never two clean ones. The malformed type resolves downstream.
    let source = "module app\nfn f(r: Map<int, x: int): int {\n    return x\n}\n";
    let parsed = parse_source(source);
    let function = parsed.file.function("f").expect("f");
    assert_eq!(
        function.params.len(),
        1,
        "the unterminated generic keeps the parameter list from splitting: {:#?}",
        function.params
    );
}

#[test]
fn a_doubled_generic_close_in_a_parameter_is_rejected() {
    // `Map<int, string>>` has an unbalanced trailing `>`; the type parser reports
    // it rather than accepting a stray token after a complete type.
    let source = "module app\nfn f(r: Map<int, string>>, x: int): int {\n    return x\n}\n";
    let parsed = parse_source(source);
    assert!(
        !parsed.diagnostics.is_empty(),
        "a doubled generic close is a parse error: {source:?}"
    );
}

// ---- T2: angle-bracket generics in every type position ----

fn const_type(source_type: &str) -> TypeExpr {
    let source =
        format!("module app\nfn run() {{\n    const x: {source_type} = y\n    return\n}}\n");
    let parsed = parse_source(&source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
    let Statement::Const { ty, .. } =
        parsed.file.function("run").unwrap().body.statements[0].clone()
    else {
        panic!("expected const");
    };
    ty.expect("a type annotation")
}

#[test]
fn single_and_multi_argument_generics_parse_to_apply() {
    for (spelling, head, arity) in [
        ("Option<string>", "Option", 1),
        ("List<int>", "List", 1),
        ("Result<bool, string>", "Result", 2),
        ("Map<K, V>", "Map", 2),
        ("Pair<A, B>", "Pair", 2),
    ] {
        let ty = const_type(spelling);
        let TypeExpr::Apply { head: h, args, .. } = ty else {
            panic!("expected a generic application for {spelling}: {ty:#?}");
        };
        assert_eq!(h, head);
        assert_eq!(args.len(), arity, "{spelling} arity");
    }
}

#[test]
fn nested_generics_close_with_two_plain_greater_tokens() {
    // `Map<string, List<int>>` closes with `> >` (no `>>` token exists).
    let ty = const_type("Map<string, List<int>>");
    let TypeExpr::Apply { head, args, .. } = &ty else {
        panic!("expected Map apply: {ty:#?}");
    };
    assert_eq!(head, "Map");
    assert_eq!(args.len(), 2);
    assert!(
        matches!(&args[1], TypeExpr::Apply { head, .. } if head == "List"),
        "second arg is List<int>: {:#?}",
        args[1]
    );
}

#[test]
fn an_optional_generic_composes_the_trailing_question() {
    // `Option<string>?` is `>` then `?` — `>?` is not a token.
    let ty = const_type("Option<string>?");
    let TypeExpr::Optional { inner, .. } = &ty else {
        panic!("expected optional: {ty:#?}");
    };
    assert!(
        matches!(&**inner, TypeExpr::Apply { head, .. } if head == "Option"),
        "inner is Option<string>: {inner:#?}"
    );
}

#[test]
fn the_canonical_generic_spelling_uses_angles() {
    assert_eq!(
        format!("{}", const_type("Map<string, List<int>>")),
        "Map<string, List<int>>"
    );
    assert_eq!(
        format!("{}", const_type("Option<string>?")),
        "Option<string>?"
    );
}

#[test]
fn generic_type_parameters_on_declarations_use_angles() {
    clean("module app\nfn identity<T>(x: T): T {\n    return x\n}\n");
    clean(
        "module app\nfn includes<T supports equality>(xs: List<T>, x: T): bool {\n    return false\n}\n",
    );
    let parsed = parse_source("module app\nstruct Pair<A, B> {\n    first: A\n    second: B\n}\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Some(Declaration::Struct(decl)) = parsed
        .file
        .declarations
        .iter()
        .find(|d| matches!(d, Declaration::Struct(_)))
    else {
        panic!("expected struct");
    };
    assert_eq!(decl.type_params.len(), 2, "Pair<A, B>");
}

#[test]
fn a_generic_type_parameter_list_still_in_brackets_is_rejected() {
    assert!(
        has_reason(
            "module app\nfn identity[T](x: T): T {\n    return x\n}\n",
            ParseDiagnosticReason::Unsupported(
                marrow_syntax::UnsupportedSyntax::UserDefinedGenerics
            ),
        ),
        "the old `[T]` generic spelling is rejected with a migration hint"
    );
}

// ---- T2: the disambiguation rule (expression `<`/`>` are always comparison) ----

#[test]
fn a_comparison_key_expression_parses() {
    // `^books[a < b]` — the key is a comparison expression, not a generic.
    let Statement::Assign { target, .. } = first_statement("fn run()", "^books[a < b] = v") else {
        panic!("expected assignment");
    };
    let Expression::Keyed { keys, .. } = target else {
        panic!("expected keyed");
    };
    assert!(
        matches!(&keys[0], Expression::Binary { .. }),
        "the key is a comparison: {:#?}",
        keys[0]
    );
}

#[test]
fn a_call_with_comparison_arguments_is_not_a_generic_application() {
    // `f(a < b, c > (d))` — two comparison arguments in a call, never `a<b,c>(d)`.
    let Statement::Expr { value, .. } = first_statement("fn run()", "f(a < b, c > (d))") else {
        panic!("expected an expression statement");
    };
    let Expression::Call { args, .. } = value else {
        panic!("expected a call: {value:#?}");
    };
    assert_eq!(args.len(), 2, "two comparison arguments");
    assert!(
        args.iter()
            .all(|a| matches!(a.value, Expression::Binary { .. }))
    );
}

#[test]
fn a_chained_comparison_stays_a_parse_error() {
    // `a < b > c` is non-associative comparison — a parse error, never a generic.
    let parsed = parse_source("module app\nfn run() {\n    const x = a < b > c\n    return\n}\n");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.reason
                == DiagnosticReason::Parser(ParseDiagnosticReason::NonAssociativeOperator)),
        "chained comparison rejects: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn the_unspaced_generic_assign_split_parses() {
    // `const m: Map<string, int>= Map()` — the `>=` glues the generic close to the
    // assignment; the one token-split the angle grammar needs.
    let parsed = parse_source(
        "module app\nfn run() {\n    const m: Map<string, int>= Map()\n    return\n}\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Statement::Const { ty, .. } =
        parsed.file.function("run").unwrap().body.statements[0].clone()
    else {
        panic!("expected const");
    };
    assert!(
        matches!(ty, Some(TypeExpr::Apply { ref head, .. }) if head == "Map"),
        "the type is Map<string, int>: {ty:#?}"
    );
}

#[test]
fn a_greater_equal_comparison_in_a_value_is_untouched() {
    // A genuine `>=` comparison in a value still parses as a comparison.
    let Statement::Const { value, .. } =
        first_statement("fn run(a: int, b: int)", "const ok: bool = a >= b\nreturn")
    else {
        panic!("expected const");
    };
    assert!(
        matches!(value, Expression::Binary { .. }),
        "a >= b is a comparison: {value:#?}"
    );
}

// ---- => match arms interacting with the angle grammar ----

#[test]
fn match_arms_coexist_with_generic_annotations_and_comparisons() {
    clean(
        "module app\nfn run(s: Shape, a: int, b: int): int {\n    const m: Map<int, int>= Map()\n    match s {\n        dot => {\n            if a >= b {\n                return 1\n            }\n            return 0\n        }\n        circle(r) => return r\n    }\n    return -1\n}\n",
    );
}
