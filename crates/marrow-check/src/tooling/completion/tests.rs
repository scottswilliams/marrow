use super::*;
use marrow_syntax::{SourceSpan, lex_source};

fn context_at(source: &str) -> (String, SourceCompletionContext) {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let lexed = lex_source(&source);
    let context = source_completion_context(&source, &lexed, offset);
    (source, context)
}

#[test]
fn classifies_source_completion_cursor_contexts() {
    let (_, context) = context_at("module shelf::app\n\npub fn f()\n    delete ^|\n");
    assert_eq!(context, SourceCompletionContext::Root);

    let (_, context) = context_at("module shelf::app\n\npub fn f()\n    const x = std::clock::|\n");
    assert_eq!(
        context,
        SourceCompletionContext::Namespace {
            qualifier: vec!["std".to_string(), "clock".to_string()]
        }
    );

    let (source, context) =
        context_at("module shelf::app\n\npub fn f(id: int)\n    const x = ^books(id).|\n");
    let SourceCompletionContext::SavedPath { receiver, span } = context else {
        panic!("expected saved-path context, got {context:?}");
    };
    assert_eq!(receiver, "^books(id)");
    assert_receiver_span(&source, span, "^books(id)");

    let (_, context) =
        context_at("module shelf::app\n\npub fn f(id: int)\n    const x = ^books(id)..|\n");
    assert_eq!(context, SourceCompletionContext::InvalidSavedPath);

    let (_, context) = context_at("module shelf::app\n\npub fn f(x: |\n");
    assert_eq!(context, SourceCompletionContext::Type);

    let (_, context) = context_at("module shelf::app\n\npub fn f(total: int)\n    return t|\n");
    assert_eq!(context, SourceCompletionContext::Bare);
}

fn assert_receiver_span(source: &str, span: SourceSpan, receiver: &str) {
    assert_eq!(&source[span.start_byte..span.end_byte], receiver);
}
