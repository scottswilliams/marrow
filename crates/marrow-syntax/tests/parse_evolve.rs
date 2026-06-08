//! Evolve blocks: each step kind parsed to the AST, transform bodies, and the
//! grammar rules that keep evolve keywords contextual and well-formed.

use marrow_syntax::{
    Declaration, ExpectedSyntax, ParseDiagnosticReason, format_expression, parse_source,
};

mod common;

use common::parse_reason;

fn evolve_decl(parsed: &marrow_syntax::ParsedSource) -> &marrow_syntax::EvolveDecl {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Evolve(decl) => Some(decl),
            _ => None,
        })
        .expect("an evolve declaration")
}

#[test]
fn evolve_block_parses_each_step_to_the_ast() {
    use marrow_syntax::EvolveStep;
    let source = "module app\n\
        evolve\n\
        \x20   rename Book.title -> Book.subtitle\n\
        \x20   default Book.author = \"unknown\"\n\
        \x20   retire ^books.byTitle\n\
        \x20   transform Book.shelf\n\
        \x20       return ^books(1).shelf\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    assert_eq!(decl.steps.len(), 4);

    match &decl.steps[0] {
        EvolveStep::Rename { from, to, .. } => {
            assert_eq!(format_expression(from), "Book.title");
            assert_eq!(format_expression(to), "Book.subtitle");
        }
        other => panic!("expected rename, got {other:#?}"),
    }
    match &decl.steps[1] {
        EvolveStep::Default { target, value, .. } => {
            assert_eq!(format_expression(target), "Book.author");
            assert_eq!(format_expression(value), "\"unknown\"");
        }
        other => panic!("expected default, got {other:#?}"),
    }
    match &decl.steps[2] {
        EvolveStep::Retire { target, .. } => {
            assert_eq!(format_expression(target), "^books.byTitle");
        }
        other => panic!("expected retire, got {other:#?}"),
    }
    match &decl.steps[3] {
        EvolveStep::Transform { target, body, .. } => {
            assert_eq!(format_expression(target), "Book.shelf");
            assert_eq!(body.statements.len(), 1);
        }
        other => panic!("expected transform, got {other:#?}"),
    }
}

#[test]
fn evolve_rename_renames_a_saved_root() {
    use marrow_syntax::EvolveStep;
    let source = "module app\nevolve\n    rename ^books -> ^archive\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    match &decl.steps[0] {
        EvolveStep::Rename { from, to, .. } => {
            assert_eq!(format_expression(from), "^books");
            assert_eq!(format_expression(to), "^archive");
        }
        other => panic!("expected rename, got {other:#?}"),
    }
}

#[test]
fn evolve_rename_without_arrow_is_reported() {
    let source = "module app\nevolve\n    rename Book.title Book.subtitle\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"
            && d.reason
                == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveStep))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_default_without_value_is_reported() {
    let source = "module app\nevolve\n    default Book.title\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_unknown_step_keyword_is_reported() {
    let source = "module app\nevolve\n    rebrand Book.title\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_contextual_words_remain_identifiers_outside_the_block() {
    // `rename`, `default`, `retire`, and `transform` are contextual, so they stay
    // usable as ordinary identifiers (here, function names) outside an evolve block.
    let source = "module app\nfn rename(): int\n    return 1\nfn retire(): int\n    return 2\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.function("rename").is_some());
    assert!(parsed.file.function("retire").is_some());
}

#[test]
fn evolve_indented_block_under_a_non_transform_step_is_reported() {
    // Only a transform carries an indented body; an indented block under rename,
    // default, or retire is a mistake the parser must flag rather than silently
    // consume.
    let source = "module app\n\
        evolve\n\
        \x20   retire Book.title\n\
        \x20       stray body line\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"
            && d.reason == parse_reason(ParseDiagnosticReason::UnexpectedIndentation)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_transform_with_a_multi_statement_body_and_a_following_declaration_parse() {
    use marrow_syntax::EvolveStep;
    let source = "module app\n\
        evolve\n\
        \x20   transform Book.shelf\n\
        \x20       const old: string = ^books(1).shelf\n\
        \x20       return old\n\
        fn after(): int\n\
        \x20   return 1\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    match &decl.steps[0] {
        EvolveStep::Transform { body, .. } => assert_eq!(body.statements.len(), 2),
        other => panic!("expected transform, got {other:#?}"),
    }
    // The declaration after the evolve block still parses.
    assert!(parsed.file.function("after").is_some());
}
