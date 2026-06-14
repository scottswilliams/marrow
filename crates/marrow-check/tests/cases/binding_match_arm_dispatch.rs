//! Match-arm member resolution in the binding index: a relative arm label such as
//! `active` resolves to its enum member through the scrutinee's inferred enum,
//! whether that scrutinee is a local, a module constant, a call result, or a loop
//! binding over a sequence or saved layer. An invalid scrutinee creates no arm
//! references, and the checked program records the scrutinee enum as a typed fact.
use crate::support;
use crate::support_binding;
use marrow_check::CheckedStmt;
use marrow_check::binding::SymbolKind;

use support::analyze_overlay;
use support_binding::{analyze, assert_def_covers_member, checked_index};

#[test]
fn a_match_arm_resolves_to_the_enum_member_definition() {
    // Match arms are relative member paths. The scrutinee's enum supplies the
    // `Status` prefix, so a cursor on `active` should still reach `Status::active`.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(s: Status): int\n    \
        match s\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-arm", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index.definition(file, arm_use).expect("match arm resolves");
    assert_def_covers_member(&def, source, "active\n");

    let refs = index.references(&def);
    assert!(
        refs.iter()
            .any(|reference| reference.span.start_byte <= arm_use
                && arm_use <= reference.span.end_byte),
        "match arm use is a reference: {refs:?}",
    );
}

#[test]
fn a_match_arm_resolves_through_an_inferred_enum_local() {
    // The checker infers `s` as `Status` from its enum-member initializer, so the
    // binding index should use that same type when resolving relative match arms.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(): int\n    \
        const s = Status::active\n    \
        match s\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-inferred-local", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from inferred local resolves");
    assert_def_covers_member(&def, source, "active\n");
}

#[test]
fn a_match_arm_resolves_through_a_module_enum_constant() {
    // Module constants are part of the checker prelude for every function body.
    // Match arm navigation should see their enum type too.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        const Default: Status = Status::active\n\
        fn classify(): int\n    \
        match Default\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-module-const", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("archived\n            return 2")
        .expect("archived match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from module constant resolves");
    assert_def_covers_member(&def, source, "archived\n");
}

#[test]
fn a_match_arm_trailing_comment_is_not_a_member_reference() {
    // The reference span should cover the member path, not trivia after it.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(s: Status): int\n    \
        match s\n        \
        active ; chosen case\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-comment-span", &[("src/m.mw", source)]);
    let file = &paths[0];

    let comment = source.find("chosen").expect("arm trailing comment");
    assert!(
        index.definition(file, comment).is_none(),
        "trailing comment text must not resolve as an enum member",
    );

    let after_label = source.find("active ;").expect("active arm") + "active".len();
    assert!(
        index.definition(file, after_label).is_none(),
        "the space after a match arm label must not resolve as an enum member",
    );
}

#[test]
fn a_match_arm_resolves_through_an_enum_returning_call() {
    // Match dispatch uses the scrutinee expression's inferred enum type, not just
    // local names. A call returning `Status` should unlock relative arm refs.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn pick(): Status\n    \
        return Status::active\n\
        fn classify(): int\n    \
        match pick()\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-call-scrutinee", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from enum-returning call resolves");
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");
}

#[test]
fn an_invalid_enum_member_scrutinee_does_not_create_arm_references() {
    // `Status::missing` names the enum prefix but no member. The checker rejects
    // the scrutinee, and the binding index should avoid false arm references.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(): int\n    \
        match Status::missing\n        \
        active\n            \
        return 1\n        \
        archived\n            \
        return 2\n";
    let (index, paths) = analyze("enum-match-invalid-scrutinee", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n            return 1")
        .expect("active match arm");
    assert!(
        index.definition(file, arm_use).is_none(),
        "invalid enum scrutinee should not create arm member refs",
    );
}

#[test]
fn a_match_arm_resolves_through_a_sequence_enum_loop_binding() {
    // Loop bindings use the checker-shared `for` frame, so iterating
    // `sequence[Status]` makes `s` a `Status` value for relative match arms.
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn classify(items: sequence[Status]): int\n    \
        for s in items\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (index, paths) = analyze("enum-match-sequence-loop", &[("src/m.mw", source)]);
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from sequence enum loop binding resolves");
    assert_def_covers_member(&def, source, "active\n");
}

#[test]
fn a_match_arm_resolves_through_a_saved_enum_layer_values_loop_binding() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book\n    \
        states(pos: int): Status\n\
        store ^books(id: int): Book\n\
        fn classify(id: Id(^books)): int\n    \
        for s in values(^books(id).states)\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (index, paths) = checked_index(
        "enum-match-saved-layer-values-loop",
        &[("src/m.mw", source)],
    );
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return 1")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from saved enum layer values loop binding resolves");
    assert_def_covers_member(&def, source, "active\n");
}

#[test]
fn a_match_arm_resolves_through_a_two_name_saved_enum_layer_loop_binding() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book\n    \
        states(pos: int): Status\n\
        store ^books(id: int): Book\n\
        fn classify(id: Id(^books)): int\n    \
        for pos, s in ^books(id).states\n        \
        match s\n            \
        active\n                \
        return pos\n            \
        archived\n                \
        return 0\n    \
        return 0\n";
    let (index, paths) = checked_index(
        "enum-match-two-name-saved-layer-loop",
        &[("src/m.mw", source)],
    );
    let file = &paths[0];

    let arm_use = source
        .rfind("active\n                return pos")
        .expect("active match arm");
    let def = index
        .definition(file, arm_use)
        .expect("match arm from two-name saved enum layer loop binding resolves");
    assert_def_covers_member(&def, source, "active\n");
}

#[test]
fn a_saved_enum_layer_loop_match_records_its_scrutinee_enum() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book\n    \
        states(pos: int): Status\n\
        store ^books(id: int): Book\n\
        fn classify(id: Id(^books)): int\n    \
        for s in values(^books(id).states)\n        \
        match s\n            \
        active\n                \
        return 1\n            \
        archived\n                \
        return 2\n    \
        return 0\n";
    let (snapshot, _) = analyze_overlay("enum-match-saved-layer-stamp", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    let function = snapshot.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == "classify")
        .expect("classify function");
    let runtime_body = function.runtime_body().expect("runtime body");
    let loop_body = runtime_body
        .statements()
        .iter()
        .find_map(|statement| match statement {
            CheckedStmt::For { body, .. } => Some(body),
            _ => None,
        })
        .expect("saved layer loop");
    let enum_ref = loop_body
        .statements()
        .iter()
        .find_map(|statement| match statement {
            CheckedStmt::Match {
                enum_ref: Some(enum_ref),
                ..
            } => Some(*enum_ref),
            _ => None,
        })
        .expect("match in loop body");
    let enum_fact = snapshot
        .program
        .facts
        .enums()
        .iter()
        .find(|fact| fact.id == enum_ref.enum_id)
        .expect("match enum is recorded in checked facts");
    let module = snapshot
        .program
        .facts
        .modules()
        .iter()
        .find(|fact| fact.id == enum_fact.module)
        .expect("enum module is recorded in checked facts");

    assert_eq!(
        (module.name.as_str(), enum_fact.name.as_str()),
        ("m", "Status")
    );
}
