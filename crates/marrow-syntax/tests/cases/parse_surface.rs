use marrow_syntax::{
    CommentPlacement, Declaration, ExpectedSyntax, ParseDiagnosticReason, SurfaceDecl, SurfaceItem,
    SurfaceTarget, parse_source,
};

fn parse_reason(reason: ParseDiagnosticReason) -> marrow_syntax::DiagnosticReason {
    marrow_syntax::DiagnosticReason::Parser(reason)
}

/// The per-name spans a field-list item carries, for asserting structural
/// equality without recomputing each name's column by hand.
fn field_name_spans(item: &SurfaceItem) -> Vec<marrow_syntax::SourceSpan> {
    match item {
        SurfaceItem::Fields { name_spans, .. }
        | SurfaceItem::Create { name_spans, .. }
        | SurfaceItem::Update { name_spans, .. } => name_spans.clone(),
        _ => panic!("not a field-list surface item: {item:?}"),
    }
}

/// The `^target` span a collection item carries, for asserting structural
/// equality without recomputing each target's column by hand.
fn collection_target_span(item: &SurfaceItem) -> marrow_syntax::SourceSpan {
    match item {
        SurfaceItem::Collection { target, .. } => target.span(),
        _ => panic!("not a collection surface item: {item:?}"),
    }
}

/// The function-target span an action/read item carries, for asserting
/// structural equality without recomputing the target's column by hand.
fn function_target_span(item: &SurfaceItem) -> marrow_syntax::SourceSpan {
    match item {
        SurfaceItem::Action { function_span, .. } | SurfaceItem::Read { function_span, .. } => {
            *function_span
        }
        _ => panic!("not an action/read surface item: {item:?}"),
    }
}

fn surface_decl(source: &str) -> SurfaceDecl {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected clean parse, got {:#?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .iter()
        .find_map(|decl| match decl {
            Declaration::Surface(surface) => Some(surface.clone()),
            _ => None,
        })
        .expect("surface declaration")
}

#[test]
fn parses_surface_declaration_with_contextual_items() {
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   fields title, author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   create title, author, blurb\n\
         \x20   update title, blurb\n\
         \x20   delete\n\
         \x20   read bookPage as page\n\
         \x20   action addBook\n\
         \x20   action shelf::loanBook as loan\n",
    );

    assert_eq!(surface.name, "Books");
    assert_eq!(surface.store.root, "books");
    assert!(surface.store.keys.is_empty());
    assert_eq!(surface.items.len(), 9);
    assert_eq!(
        surface.items[0],
        SurfaceItem::Fields {
            names: vec!["title".into(), "author".into(), "blurb".into()],
            name_spans: field_name_spans(&surface.items[0]),
            span: surface.items[0].span(),
        }
    );
    assert_eq!(
        surface.items[1],
        SurfaceItem::Collection {
            target: SurfaceTarget::Root {
                root: "books".into(),
                span: collection_target_span(&surface.items[1]),
            },
            alias: "list".into(),
            span: surface.items[1].span(),
        }
    );
    assert_eq!(
        surface.items[2],
        SurfaceItem::Collection {
            target: SurfaceTarget::Index {
                root: "books".into(),
                index: "byAuthor".into(),
                span: collection_target_span(&surface.items[2]),
            },
            alias: "byAuthor".into(),
            span: surface.items[2].span(),
        }
    );
    assert_eq!(
        surface.items[3],
        SurfaceItem::Create {
            names: vec!["title".into(), "author".into(), "blurb".into()],
            name_spans: field_name_spans(&surface.items[3]),
            span: surface.items[3].span(),
        }
    );
    assert_eq!(
        surface.items[4],
        SurfaceItem::Update {
            names: vec!["title".into(), "blurb".into()],
            name_spans: field_name_spans(&surface.items[4]),
            span: surface.items[4].span(),
        }
    );
    assert_eq!(
        surface.items[5],
        SurfaceItem::Delete {
            span: surface.items[5].span(),
        }
    );
    assert_eq!(
        surface.items[6],
        SurfaceItem::Read {
            function: vec!["bookPage".into()],
            function_span: function_target_span(&surface.items[6]),
            alias: "page".into(),
            span: surface.items[6].span(),
        }
    );
    assert_eq!(
        surface.items[7],
        SurfaceItem::Action {
            function: vec!["addBook".into()],
            function_span: function_target_span(&surface.items[7]),
            alias: "addBook".into(),
            span: surface.items[7].span(),
        }
    );
    assert_eq!(
        surface.items[8],
        SurfaceItem::Action {
            function: vec!["shelf".into(), "loanBook".into()],
            function_span: function_target_span(&surface.items[8]),
            alias: "loan".into(),
            span: surface.items[8].span(),
        }
    );
}

#[test]
fn field_list_items_record_each_name_span() {
    // Each field name on a `fields`/`create`/`update` line carries its own span so a
    // checker rejection points at the offending name rather than column 1.
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   fields title, author\n",
    );
    let spans = field_name_spans(&surface.items[0]);
    assert_eq!(spans.len(), 2, "{:#?}", surface.items[0]);
    assert_eq!((spans[0].line, spans[0].column), (3, 12));
    assert_eq!((spans[1].line, spans[1].column), (3, 19));
}

#[test]
fn collection_items_record_the_target_token_span() {
    // The `^target` of a collection line carries its own span so a checker
    // rejection points at the offending target rather than column 1.
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   collection ^shelf.byAuthor as list\n",
    );
    let span = collection_target_span(&surface.items[0]);
    assert_eq!((span.line, span.column), (3, 16));
}

#[test]
fn action_and_read_items_record_the_function_target_token_span() {
    // The function target of an `action`/`read` line carries its own span so a
    // checker rejection points at the offending target rather than column 1.
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   action shelf::loanBook as loan\n\
         \x20   read bookPage\n",
    );
    let action_span = function_target_span(&surface.items[0]);
    assert_eq!((action_span.line, action_span.column), (3, 12));
    let read_span = function_target_span(&surface.items[1]);
    assert_eq!((read_span.line, read_span.column), (4, 10));
}

#[test]
fn surface_contextual_words_remain_identifiers_outside_surface_blocks() {
    let parsed = parse_source(
        "module app\n\
         const from = 1\n\
         fn fields(collection: int)\n\
         \x20   const create = collection\n\
         \x20   return\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.function("fields").is_some());
}

#[test]
fn surface_collection_index_can_be_named_as() {
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   collection ^books.as as byAs\n",
    );

    assert_eq!(surface.items.len(), 1);
    assert_eq!(
        surface.items[0],
        SurfaceItem::Collection {
            target: SurfaceTarget::Index {
                root: "books".into(),
                index: "as".into(),
                span: collection_target_span(&surface.items[0]),
            },
            alias: "byAs".into(),
            span: surface.items[0].span(),
        }
    );
}

#[test]
fn reports_surface_body_when_indented_body_has_no_items() {
    let cases = [
        "module app\n\
         surface Books from ^books\n\
         \x20   ; comment-only body\n",
        "module app\n\
         surface Books from ^books\n\
         \x20   \n",
    ];
    for source in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.reason
                    == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceBody))
            }),
            "expected SurfaceBody for {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn reports_malformed_surface_header_and_items() {
    let cases = [
        (
            "module app\nsurface Books ^books\n",
            ExpectedSyntax::SurfaceHeader,
        ),
        (
            "module app\nsurface Books from books\n",
            ExpectedSyntax::SurfaceStore,
        ),
        (
            "module app\nsurface Books from ^books\n    fields\n",
            ExpectedSyntax::SurfaceFieldList,
        ),
        (
            "module app\nsurface Books from ^books\n    collection ^books\n",
            ExpectedSyntax::SurfaceCollection,
        ),
        (
            "module app\nsurface Books from ^books\n    collection ^books as\n",
            ExpectedSyntax::SurfaceCollection,
        ),
        (
            "module app\nsurface Books from ^books\n    action\n",
            ExpectedSyntax::SurfaceAction,
        ),
        (
            "module app\nsurface Books from ^books\n    action shelf::loan as\n",
            ExpectedSyntax::SurfaceAction,
        ),
        (
            "module app\nsurface Books from ^books\n    read\n",
            ExpectedSyntax::SurfaceRead,
        ),
        (
            "module app\nsurface Books from ^books\n    read shelf::page as\n",
            ExpectedSyntax::SurfaceRead,
        ),
        (
            "module app\nsurface Books from ^books\n    bogus title\n",
            ExpectedSyntax::SurfaceItem,
        ),
        (
            "module app\nsurface Books from ^books\n    delete title\n",
            ExpectedSyntax::SurfaceItem,
        ),
    ];
    for (source, expected) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            }),
            "expected {expected:?} for {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn malformed_surface_items_do_not_also_report_missing_body() {
    let cases = [
        (
            "module app\nsurface Books from ^books\n    fields\n",
            ExpectedSyntax::SurfaceFieldList,
        ),
        (
            "module app\nsurface Books from ^books\n    collection ^books\n",
            ExpectedSyntax::SurfaceCollection,
        ),
        (
            "module app\nsurface Books from ^books\n    collection ^books as\n",
            ExpectedSyntax::SurfaceCollection,
        ),
        (
            "module app\nsurface Books from ^books\n    action\n",
            ExpectedSyntax::SurfaceAction,
        ),
        (
            "module app\nsurface Books from ^books\n    read\n",
            ExpectedSyntax::SurfaceRead,
        ),
        (
            "module app\nsurface Books from ^books\n    bogus title\n",
            ExpectedSyntax::SurfaceItem,
        ),
        (
            "module app\nsurface Books from ^books\n    delete title\n",
            ExpectedSyntax::SurfaceItem,
        ),
    ];
    for (source, expected) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            }),
            "expected {expected:?} for {source:?}: {:#?}",
            parsed.diagnostics
        );
        assert!(
            parsed.diagnostics.iter().all(|diagnostic| {
                diagnostic.reason
                    != parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceBody))
            }),
            "did not expect SurfaceBody for {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_surface_collection_targets_that_are_not_source_native_root_or_index_paths() {
    let parsed = parse_source(
        "module app\n\
         surface Books from ^books\n\
         \x20   collection books as list\n\
         \x20   collection ^books.byAuthor.extra as bad\n",
    );

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::SurfaceCollectionTarget,
                ))
        }),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn preserves_surface_body_comments() {
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   ; public fields\n\
         \x20   fields title ; title only\n\
         \x20   collection ^books as list ; list collection\n",
    );

    assert_eq!(surface.comments.len(), 3);
    assert_eq!(surface.comments[0].placement, CommentPlacement::OwnLine);
    assert_eq!(surface.comments[0].text, "public fields");
    assert_eq!(surface.comments[1].placement, CommentPlacement::Trailing);
    assert_eq!(surface.comments[1].text, "title only");
    assert_eq!(surface.comments[2].placement, CommentPlacement::Trailing);
    assert_eq!(surface.comments[2].text, "list collection");
}
