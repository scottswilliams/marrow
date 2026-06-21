use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    CallableArgumentStyle, CallableValueShape, SourceSignatureHelpCallable,
    SourceSignatureHelpFact, SourceSignatureHelpParameter, source_signature_help_fact_at,
};
use marrow_check::{AnalysisSnapshot, MarrowType, ScalarType};
use marrow_syntax::lex_source;

fn analyze_files(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    (snapshot, paths)
}

fn analyze_files_with_diagnostics(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, Vec<PathBuf>) {
    support::analyze_overlay(name, files)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    source_with_cursor: &str,
) -> Option<SourceSignatureHelpFact> {
    let offset = source_with_cursor
        .find('|')
        .expect("source contains cursor marker");
    let source = source_with_cursor.replacen('|', "", 1);
    let lexed = lex_source(&source);
    source_signature_help_fact_at(
        &snapshot.program,
        Some(snapshot),
        file,
        &source,
        &lexed,
        offset,
    )
}

fn int_ty() -> MarrowType {
    MarrowType::Primitive(ScalarType::Int)
}

fn string_ty() -> MarrowType {
    MarrowType::Primitive(ScalarType::Str)
}

fn assert_no_fact(snapshot: &AnalysisSnapshot, file: &Path, source_with_cursor: &str) {
    assert_eq!(
        fact_at(snapshot, file, source_with_cursor),
        None,
        "expected no signature help for {source_with_cursor:?}"
    );
}

#[test]
fn source_signature_help_fact_resolves_imported_user_function_docs_and_active_parameter() {
    let math = "\
module shelf::math

;; Adds two values.
pub fn add(
    ;; Left value.
    left: int,
    ;; Right value.
    right: int,
): int
    return left + right
";
    let app = "\
module shelf::app

use shelf::math

pub fn run(): int
    return math::add(1, 2)
";
    let (snapshot, paths) = analyze_files(
        "source-signature-help-user-function",
        &[("src/shelf/math.mw", math), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];

    let fact = fact_at(
        &snapshot,
        file,
        "module shelf::app\n\nuse shelf::math\n\npub fn run(): int\n    return math::add(1, |\n",
    )
    .expect("signature help fact");

    assert_eq!(fact.active_argument, 1);
    assert_eq!(fact.named_argument, None);
    assert_eq!(
        fact.callable,
        SourceSignatureHelpCallable::Function {
            name: "add".to_string(),
            docs: vec!["Adds two values.".to_string()],
            params: vec![
                SourceSignatureHelpParameter {
                    name: Some("left".to_string()),
                    label: "left".to_string(),
                    required: true,
                    repeat: false,
                    ty: Some(int_ty()),
                    shape: None,
                    docs: vec!["Left value.".to_string()],
                },
                SourceSignatureHelpParameter {
                    name: Some("right".to_string()),
                    label: "right".to_string(),
                    required: true,
                    repeat: false,
                    ty: Some(int_ty()),
                    shape: None,
                    docs: vec!["Right value.".to_string()],
                },
            ],
            return_type: Some(int_ty()),
        }
    );
}

#[test]
fn source_signature_help_fact_resolves_resource_constructor_named_argument_fields() {
    let books = "\
module shelf::books

;; Publication state.
pub enum Status
    active

;; Reader-visible review.
resource Review
    ;; Review state.
    status: Status
    tags(pos: int): string
";
    let app = "\
module shelf::app

use shelf::books

pub fn run(): books::Review
    return books::Review(status: books::Status::active)
";
    let (snapshot, paths) = analyze_files(
        "source-signature-help-resource-constructor",
        &[("src/shelf/books.mw", books), ("src/shelf/app.mw", app)],
    );
    let file = &paths[1];

    let fact = fact_at(
        &snapshot,
        file,
        "module shelf::app\n\nuse shelf::books\n\npub fn run(): books::Review\n    return books::Review(status: |\n",
    )
    .expect("signature help fact");

    assert_eq!(fact.active_argument, 0);
    assert_eq!(fact.named_argument, Some("status".to_string()));
    assert_eq!(
        fact.callable,
        SourceSignatureHelpCallable::ResourceConstructor {
            name: "Review".to_string(),
            docs: vec!["Reader-visible review.".to_string()],
            params: vec![SourceSignatureHelpParameter {
                name: Some("status".to_string()),
                label: "status".to_string(),
                required: false,
                repeat: false,
                ty: Some(MarrowType::Enum {
                    module: "shelf::books".to_string(),
                    name: "Status".to_string(),
                }),
                shape: None,
                docs: vec!["Review state.".to_string()],
            }],
            return_type: MarrowType::Resource("shelf::books::Review".to_string()),
        }
    );
}

#[test]
fn source_signature_help_fact_fails_closed_for_mixed_std_project_duplicate_alias() {
    let text = "\
module my::text

pub fn contains(value: int): bool
    return true
";
    let app = "\
module app

use std::text
use my::text

pub fn run(): bool
    return text::contains(1)
";
    let (snapshot, paths) = analyze_files_with_diagnostics(
        "source-signature-help-mixed-duplicate-alias",
        &[("src/my/text.mw", text), ("src/app.mw", app)],
    );

    assert_no_fact(
        &snapshot,
        &paths[1],
        "module app\n\nuse std::text\nuse my::text\n\npub fn run(): bool\n    return text::contains(|\n",
    );
}

#[test]
fn source_signature_help_fact_fails_closed_for_duplicate_project_aliases_before_fallback() {
    let first = "\
module first::api

pub fn make(value: int): int
    return value

resource Badge
    value: int
";
    let second = "\
module second::api

pub fn make(value: int): int
    return value

resource Badge
    value: int
";
    let app = "\
module app

use first::api
use second::api

pub fn run(): int
    return api::make(1)
";
    let (snapshot, paths) = analyze_files_with_diagnostics(
        "source-signature-help-project-duplicate-alias",
        &[
            ("src/first/api.mw", first),
            ("src/second/api.mw", second),
            ("src/app.mw", app),
        ],
    );
    let file = &paths[2];

    assert_no_fact(
        &snapshot,
        file,
        "module app\n\nuse first::api\nuse second::api\n\npub fn run(): int\n    return api::make(|\n",
    );
    assert_no_fact(
        &snapshot,
        file,
        "module app\n\nuse first::api\nuse second::api\n\npub fn run(): first::api::Badge\n    return api::Badge(|\n",
    );
}

#[test]
fn source_signature_help_fact_fails_closed_for_project_alias_local_collision() {
    let imported = "\
module my::tools

pub fn make(value: int): int
    return value

resource Badge
    value: int
";
    let app = "\
module app

use my::tools

resource tools
    value: int

pub fn run(): int
    return tools::make(1)
";
    let (snapshot, paths) = analyze_files_with_diagnostics(
        "source-signature-help-project-alias-local-collision",
        &[("src/my/tools.mw", imported), ("src/app.mw", app)],
    );
    let file = &paths[1];

    assert_no_fact(
        &snapshot,
        file,
        "module app\n\nuse my::tools\n\nresource tools\n    value: int\n\npub fn run(): int\n    return tools::make(|\n",
    );
    assert_no_fact(
        &snapshot,
        file,
        "module app\n\nuse my::tools\n\nresource tools\n    value: int\n\npub fn run(): my::tools::Badge\n    return tools::Badge(|\n",
    );
}

#[test]
fn source_signature_help_fact_expands_imported_std_alias_and_fails_closed_on_collision() {
    let source = "\
module app

use std::text

pub fn run(): bool
    return text::contains(\"abc\", \"b\")
";
    let (snapshot, paths) =
        analyze_files("source-signature-help-std-alias", &[("src/app.mw", source)]);
    let fact = fact_at(
        &snapshot,
        &paths[0],
        "module app\n\nuse std::text\n\npub fn run(): bool\n    return text::contains(\"abc\", |\n",
    )
    .expect("signature help fact");

    assert_eq!(fact.active_argument, 1);
    assert_eq!(
        fact.callable,
        SourceSignatureHelpCallable::Intrinsic {
            path: vec![
                "std".to_string(),
                "text".to_string(),
                "contains".to_string(),
            ],
            argument_style: CallableArgumentStyle::Positional,
            docs: Vec::new(),
            params: vec![
                SourceSignatureHelpParameter {
                    name: None,
                    label: "string".to_string(),
                    required: true,
                    repeat: false,
                    ty: Some(string_ty()),
                    shape: Some(CallableValueShape::Type(string_ty())),
                    docs: Vec::new(),
                },
                SourceSignatureHelpParameter {
                    name: None,
                    label: "string".to_string(),
                    required: true,
                    repeat: false,
                    ty: Some(string_ty()),
                    shape: Some(CallableValueShape::Type(string_ty())),
                    docs: Vec::new(),
                },
            ],
            return_shape: Some(CallableValueShape::Type(MarrowType::Primitive(
                ScalarType::Bool
            ))),
        }
    );

    let collision = "\
module app

use std::text

resource text
    value: string

pub fn run(): bool
    return text::contains(\"abc\", \"b\")
";
    let (snapshot, paths) = analyze_files_with_diagnostics(
        "source-signature-help-std-alias-collision",
        &[("src/app.mw", collision)],
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &paths[0],
            "module app\n\nuse std::text\n\nresource text\n    value: string\n\npub fn run(): bool\n    return text::contains(\"abc\", |\n",
        ),
        None
    );
}

#[test]
fn source_signature_help_fact_returns_none_for_non_call_contexts() {
    let source = "\
module app

resource Book
    title: string
    tags(pos: int): string

store ^books(id: int): Book

fn calc(value: int): int
    return value

pub fn run(book: Book): int
    return calc(1)
";
    let (snapshot, paths) =
        analyze_files("source-signature-help-absence", &[("src/app.mw", source)]);
    let file = &paths[0];

    for source_with_cursor in [
        "module app\n\nfn calc(|value: int): int\n    return value\n",
        "module app\n\nconst typed: calc(|1) = 1\n",
        "module app\n\nresource Book\n    tags(|pos: int): string\n",
        "module app\n\nstore ^books(|id: int): Book\n",
        "module app\n\npub fn run(book: Book): string\n    return book.title(|1)\n",
    ] {
        assert_eq!(
            fact_at(&snapshot, file, source_with_cursor),
            None,
            "expected no signature help for {source_with_cursor:?}"
        );
    }
}
