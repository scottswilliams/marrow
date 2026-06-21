use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    SourceCallableHoverFact, SourceCallableParamFact, source_callable_hover_fact_at,
    source_symbol_docs_at,
};
use marrow_check::{AnalysisSnapshot, BindingIndex, MarrowType, ScalarType, build_binding_index};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, BindingIndex, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths[0].clone())
}

fn analyze_files(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceCallableHoverFact> {
    source_callable_hover_fact_at(snapshot, index, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn int_ty() -> MarrowType {
    MarrowType::Primitive(ScalarType::Int)
}

fn string_ty() -> MarrowType {
    MarrowType::Primitive(ScalarType::Str)
}

#[test]
fn source_callable_hover_fact_covers_function_call_leaf() {
    let source = "\
module a

;; Adds two numbers.
pub fn add(x: int, y: int): int
    return x + y

pub fn caller(): int
    return add(1, 2)
";
    let (snapshot, index, file) = analyze("source-callable-hover-function-call", source);
    let fact = fact_at(&snapshot, &index, &file, offset(source, "add(1, 2)") + 1)
        .expect("function hover fact");

    let SourceCallableHoverFact::Function(function) = fact else {
        panic!("expected function fact");
    };
    assert_eq!(function.name, "add");
    assert_eq!(function.docs, vec!["Adds two numbers.".to_string()]);
    assert_eq!(function.return_type, Some(int_ty()));
    assert_eq!(
        function.params,
        vec![
            SourceCallableParamFact {
                name: "x".to_string(),
                ty: int_ty(),
                docs: Vec::new(),
            },
            SourceCallableParamFact {
                name: "y".to_string(),
                ty: int_ty(),
                docs: Vec::new(),
            },
        ]
    );
}

#[test]
fn source_callable_hover_fact_covers_function_declaration_name() {
    let source = "\
module a

;; Adds two numbers.
pub fn add(x: int, y: int): int
    return x + y
";
    let (snapshot, index, file) = analyze("source-callable-hover-function-declaration", source);
    let fact = fact_at(&snapshot, &index, &file, offset(source, "add(x") + 1)
        .expect("function declaration hover fact");

    let SourceCallableHoverFact::Function(function) = fact else {
        panic!("expected function fact");
    };
    assert_eq!(function.name, "add");
    assert_eq!(function.docs, vec!["Adds two numbers.".to_string()]);
    assert_eq!(function.return_type, Some(int_ty()));
    assert_eq!(function.params.len(), 2);
}

#[test]
fn source_callable_hover_fact_covers_qualified_function_call_leaf_only() {
    let math = "\
module shelf::math

;; Adds two numbers.
pub fn add(x: int, y: int): int
    return x + y
";
    let app = "\
module shelf::app

use shelf::math

pub fn caller(): int
    return math::add(1, 2)
";
    let (snapshot, index, paths) = analyze_files(
        "source-callable-hover-qualified-function-leaf",
        &[("src/shelf/math.mw", math), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];
    let call = offset(app, "math::add(1, 2)");

    assert_eq!(fact_at(&snapshot, &index, file, call + 1), None);
    let fact = fact_at(&snapshot, &index, file, call + "math::".len() + 1)
        .expect("function leaf hover fact");
    let SourceCallableHoverFact::Function(function) = fact else {
        panic!("expected function fact");
    };
    assert_eq!(function.name, "add");
    assert_eq!(function.docs, vec!["Adds two numbers.".to_string()]);
}

#[test]
fn source_callable_hover_fact_covers_cross_file_call_leaf_with_matching_span() {
    let math_base = "\
module shelf::math

;; Adds two numbers.
pub fn add(x: int, y: int): int
    return x + y
";
    let app = "\
module shelf::app
use shelf::math
pub fn caller()
 math::add(1, 2)
";
    let call_leaf = offset(app, "math::add(1, 2)") + "math::".len();
    let declaration_name = offset(math_base, "add(x");
    let padding = "x".repeat(
        call_leaf
            .checked_sub(declaration_name)
            .expect("call leaf is after declaration name in this fixture"),
    );
    let docs = format!("Adds two numbers.{padding}");
    let math = format!(
        "\
module shelf::math

;; {docs}
pub fn add(x: int, y: int): int
    return x + y
"
    );
    assert_eq!(offset(&math, "add(x"), call_leaf);

    let (snapshot, index, paths) = analyze_files(
        "source-callable-hover-cross-file-matching-span",
        &[
            ("src/shelf/math.mw", math.as_str()),
            ("src/shelf/app.mw", app),
        ],
    );
    let app_file = &paths[1];

    let fact = fact_at(&snapshot, &index, app_file, call_leaf + 1)
        .expect("function hover fact for aligned cross-file call");
    let SourceCallableHoverFact::Function(function) = fact else {
        panic!("expected function fact");
    };
    assert_eq!(function.name, "add");
    assert_eq!(function.docs, vec![docs.clone()]);
    assert_eq!(
        source_symbol_docs_at(&snapshot, &index, app_file, call_leaf + 1).map(|docs| docs.lines),
        Some(vec![docs])
    );
}

#[test]
fn source_callable_hover_fact_carries_direct_effect_facts() {
    let source = "\
module a

resource Book
    required title: string
    required visits: int

store ^books(id: int): Book

pub fn touch(id: int): string
    const title: string = ^books(id).title ?? \"\"
    const visits: int = ^books(id).visits ?? 0
    transaction
        ^books(id).visits = visits + 1
    print(title)
    return title

pub fn caller(): string
    return touch(1)
";
    let (snapshot, index, file) = analyze("source-callable-hover-function-effects", source);
    let fact = fact_at(&snapshot, &index, &file, offset(source, "touch(1)") + 1)
        .expect("function hover fact");

    let SourceCallableHoverFact::Function(function) = fact else {
        panic!("expected function fact");
    };
    let effects = function.direct_effects.expect("direct effects");
    assert_eq!(effects.saved_reads.len(), 2);
    assert_eq!(effects.saved_writes.len(), 1);
    assert!(effects.transactions);
    assert_eq!(effects.host_calls.len(), 1);
}

#[test]
fn source_callable_hover_fact_covers_parameter_uses_only() {
    let source = "\
module a

resource Book
    required title: string

pub fn get(
    ;; Title parameter.
    title: string,
): string
    const copied = title
    const book = Book(title: title)
    return book.title
";
    let (snapshot, index, file) = analyze("source-callable-hover-parameter-use", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "title: string")),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "Book(title: title)") + "Book(".len()
        ),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "book.title") + "book.".len()
        ),
        None
    );

    let fact = fact_at(
        &snapshot,
        &index,
        &file,
        offset(source, "copied = title") + "copied = ".len() + 1,
    )
    .expect("parameter-use hover fact");
    assert_eq!(
        fact,
        SourceCallableHoverFact::Parameter(SourceCallableParamFact {
            name: "title".to_string(),
            ty: string_ty(),
            docs: vec!["Title parameter.".to_string()],
        })
    );
}

#[test]
fn source_callable_hover_fact_uses_the_parameter_binding_in_scope() {
    let source = "\
module a

pub fn f(
    ;; First parameter docs.
    n: int,
    ;; Second parameter docs.
    n: string,
): string
    return n
";
    let (snapshot, index, file) = analyze("source-callable-hover-duplicate-parameter", source);

    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "return n") + "return ".len(),
        ),
        Some(SourceCallableHoverFact::Parameter(
            SourceCallableParamFact {
                name: "n".to_string(),
                ty: string_ty(),
                docs: vec!["Second parameter docs.".to_string()],
            }
        ))
    );
}

#[test]
fn source_callable_hover_fact_returns_none_for_shadowing_local_use() {
    let source = "\
module a

pub fn f(
    ;; Parameter docs.
    n: int,
): int
    if true
        const n: int = 1
        return n
    return n
";
    let (snapshot, index, file) = analyze("source-callable-hover-shadowing-local", source);

    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "return n") + "return ".len(),
        ),
        None
    );
}

#[test]
fn source_callable_hover_fact_covers_module_const_declaration_name_only() {
    let source = "\
module a

;; Maximum count.
const LIMIT: int = 10

pub fn caller(): int
    return LIMIT
";
    let (snapshot, index, file) = analyze("source-callable-hover-module-const", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "LIMIT: int") + 1),
        Some(SourceCallableHoverFact::ModuleConst {
            name: "LIMIT".to_string(),
            ty: Some(int_ty()),
            docs: vec!["Maximum count.".to_string()],
        })
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "return LIMIT") + "return ".len() + 1,
        ),
        None
    );
}

#[test]
fn source_callable_hover_fact_uses_half_open_name_boundaries() {
    let source = "\
module a

pub fn add(left: int): int
    const LIMIT: int = left
    return add(LIMIT)
";
    let (snapshot, index, file) = analyze("source-callable-hover-half-open", source);

    for (label, offset) in [
        (
            "function declaration",
            offset(source, "add(left") + "add".len(),
        ),
        (
            "parameter use",
            offset(source, "= left") + "= ".len() + "left".len(),
        ),
        (
            "module const declaration",
            offset(source, "LIMIT: int") + "LIMIT".len(),
        ),
        ("function call", offset(source, "add(LIMIT)") + "add".len()),
    ] {
        assert_eq!(
            fact_at(&snapshot, &index, &file, offset),
            None,
            "{label} token end is outside the source callable hover name"
        );
    }
}
