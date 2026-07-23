//! Parser-owned spans for declaration, binding, type, index, and call sites.

use marrow_syntax::{
    CheckedBind, Declaration, Expression, SourceSpan, Statement, TypeExpr, format_source,
    parse_source,
};

fn assert_site(source: &str, span: SourceSpan, spelling: &str) {
    assert!(
        span.start_byte < span.end_byte,
        "{spelling:?} must have a non-empty span: {span:?}"
    );
    assert!(
        span.end_byte <= source.len(),
        "{spelling:?} lies outside the source: {span:?}"
    );
    assert!(
        source.is_char_boundary(span.start_byte) && source.is_char_boundary(span.end_byte),
        "{spelling:?} does not lie on UTF-8 boundaries: {span:?}"
    );
    assert_eq!(
        &source[span.start_byte..span.end_byte],
        spelling,
        "span does not select the recorded token spelling"
    );

    let before = &source[..span.start_byte];
    let expected_line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let expected_column = before
        .rsplit_once('\n')
        .map_or(before.len(), |(_, line)| line.len()) as u32
        + 1;
    assert_eq!(span.line, expected_line, "wrong line for {spelling:?}");
    assert_eq!(
        span.column, expected_column,
        "wrong byte column for {spelling:?}"
    );
}

fn assert_segments(source: &str, spans: &[SourceSpan], spellings: &[&str]) {
    assert_eq!(
        spans.len(),
        spellings.len(),
        "segment spans must be parallel to the stored path"
    );
    for (span, spelling) in spans.iter().zip(spellings) {
        assert_site(source, *span, spelling);
    }
    assert!(
        spans
            .windows(2)
            .all(|pair| pair[0].end_byte <= pair[1].start_byte),
        "segment spans must retain source order: {spans:?}"
    );
}

fn assert_within(outer: SourceSpan, inner: SourceSpan, subject: &str) {
    assert!(
        outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte,
        "{subject} span {inner:?} is outside its owning node {outer:?}"
    );
}

fn assert_name_type(source: &str, ty: &TypeExpr, spelling: &str, segments: &[&str]) {
    let TypeExpr::Name {
        text,
        segment_spans,
        span,
    } = ty
    else {
        panic!("expected a name type, got {ty:?}");
    };
    assert_eq!(text, spelling);
    assert_site(source, *span, spelling);
    assert_segments(source, segment_spans, segments);
    for segment in segment_spans {
        assert_within(*span, *segment, "type-name segment");
    }
}

#[test]
fn parser_retains_every_semantic_site_span() {
    let source = concat!(
        "module app::semantic\n",
        "\n",
        "use std::bytes\n",
        "\n",
        "// 😀 keeps later byte offsets distinct from character offsets\n",
        "const Limit: int = 7\n",
        "\n",
        "resource Item {\n",
        "    value: int\n",
        "}\n",
        "\n",
        "store ^items[id: int]: Item {\n",
        "    index byValue[value] unique\n",
        "    index byCode[value, meta.code]\n",
        "}\n",
        "\n",
        "fn probe(input: Map<string, List<int>>, table[key: bytes]: int, qualified: app::Thing, odd: Foo(bar)) {\n",
        "    const local: int = 1\n",
        "    var mutable[slot: int]: bytes\n",
        "    if const present: int = ^items[1].value {\n",
        "        print(present)\n",
        "    }\n",
        "    if const first = ^items[1].value and const second = ^items[2].value {\n",
        "        print(first)\n",
        "    }\n",
        "    const fallback = ^items[1].value else {\n",
        "        return\n",
        "    }\n",
        "    var fallbackVar = ^items[1].value else {\n",
        "        return\n",
        "    }\n",
        "    const checkedConst: int = checked local + 1\n",
        "        on out_of_range {\n",
        "            return\n",
        "        }\n",
        "    var checkedVar = checked local / 1\n",
        "        on zero_divisor {\n",
        "            return\n",
        "        }\n",
        "    call(local, named: mutable)\n",
        "}\n",
    );
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(format_source(source), source);
    assert_eq!(format_source(&format_source(source)), source);

    let module = parsed.file.module.as_ref().expect("module");
    assert_eq!(module.name, "app::semantic");
    assert_segments(source, &module.segment_spans, &["app", "semantic"]);
    for segment in &module.segment_spans {
        assert_within(module.span, *segment, "module segment");
    }

    let [import] = parsed.file.uses.as_slice() else {
        panic!("expected one import");
    };
    assert_eq!(import.name, "std::bytes");
    assert_segments(source, &import.segment_spans, &["std", "bytes"]);
    for segment in &import.segment_spans {
        assert_within(import.span, *segment, "import segment");
    }

    let [
        Declaration::Const(top_const),
        Declaration::Resource(_),
        Declaration::Store(store),
        Declaration::Function(probe),
    ] = parsed.file.declarations.as_slice()
    else {
        panic!(
            "unexpected declaration corpus: {:#?}",
            parsed.file.declarations
        );
    };
    assert_site(source, top_const.name_span, "Limit");
    assert_within(top_const.span, top_const.name_span, "top-level const name");
    assert_name_type(
        source,
        top_const.ty.as_ref().expect("top-level const type"),
        "int",
        &["int"],
    );

    assert_site(source, store.resource_span, "Item");
    assert_within(store.span, store.resource_span, "store resource");
    let [root_key] = store.root.keys.as_slice() else {
        panic!("expected one root key");
    };
    assert_site(source, root_key.name_span, "id");
    assert_within(store.span, root_key.name_span, "store root key");
    assert_name_type(source, &root_key.ty, "int", &["int"]);
    assert_eq!(store.indexes.len(), 2);
    for index in &store.indexes {
        assert_eq!(index.args.len(), index.arg_spans.len());
        assert_eq!(index.args.len(), index.arg_segment_spans.len());
    }
    assert_eq!(store.indexes[0].args, ["value"]);
    assert_site(source, store.indexes[0].arg_spans[0], "value");
    assert_segments(source, &store.indexes[0].arg_segment_spans[0], &["value"]);
    assert_within(
        store.indexes[0].arg_spans[0],
        store.indexes[0].arg_segment_spans[0][0],
        "single-segment index argument",
    );
    assert_eq!(store.indexes[1].args, ["value", "meta.code"]);
    assert_site(source, store.indexes[1].arg_spans[0], "value");
    assert_segments(source, &store.indexes[1].arg_segment_spans[0], &["value"]);
    assert_within(
        store.indexes[1].arg_spans[0],
        store.indexes[1].arg_segment_spans[0][0],
        "first multi-argument index path",
    );
    assert_site(source, store.indexes[1].arg_spans[1], "meta.code");
    assert_segments(
        source,
        &store.indexes[1].arg_segment_spans[1],
        &["meta", "code"],
    );
    for segment in &store.indexes[1].arg_segment_spans[1] {
        assert_within(
            store.indexes[1].arg_spans[1],
            *segment,
            "dotted index-path segment",
        );
    }

    let [input, table, qualified, odd] = probe.params.as_slice() else {
        panic!("expected four parameters");
    };
    assert_site(source, input.name_span, "input");
    assert_within(probe.span, input.name_span, "ordinary parameter");
    let TypeExpr::Apply {
        head,
        head_span,
        args,
        ..
    } = &input.ty
    else {
        panic!("input must have a generic type");
    };
    assert_eq!(head, "Map");
    assert_site(source, *head_span, "Map");
    assert_within(input.ty.span(), *head_span, "outer generic head");
    let [key, value] = args.as_slice() else {
        panic!("Map must retain two arguments");
    };
    assert_name_type(source, key, "string", &["string"]);
    let TypeExpr::Apply {
        head,
        head_span,
        args,
        ..
    } = value
    else {
        panic!("Map value must be a nested generic");
    };
    assert_eq!(head, "List");
    assert_site(source, *head_span, "List");
    assert_within(value.span(), *head_span, "nested generic head");
    let [element] = args.as_slice() else {
        panic!("List must retain one argument");
    };
    assert_name_type(source, element, "int", &["int"]);

    assert_site(source, table.name_span, "table");
    assert_within(probe.span, table.name_span, "keyed parameter");
    let [table_key] = table.keys.as_slice() else {
        panic!("expected one local collection key");
    };
    assert_site(source, table_key.name_span, "key");
    assert_within(probe.span, table_key.name_span, "parameter key");
    assert_name_type(source, &table_key.ty, "bytes", &["bytes"]);
    assert_name_type(source, &table.ty, "int", &["int"]);

    assert_site(source, qualified.name_span, "qualified");
    assert_within(probe.span, qualified.name_span, "qualified-type parameter");
    assert_name_type(source, &qualified.ty, "app::Thing", &["app", "Thing"]);

    assert_site(source, odd.name_span, "odd");
    assert_within(probe.span, odd.name_span, "non-name-type parameter");
    let TypeExpr::Name {
        text,
        segment_spans,
        span,
    } = &odd.ty
    else {
        panic!("unresolved parenthesized spelling must remain one name type");
    };
    assert_eq!(text, "Foo(bar)");
    assert_site(source, *span, "Foo(bar)");
    assert_within(probe.span, *span, "non-name-shaped type");
    assert!(
        segment_spans.is_empty(),
        "a non-name-shaped spelling must not fabricate an identifier segment"
    );

    let statements = &probe.body.statements;
    let Statement::Const {
        name, name_span, ..
    } = &statements[0]
    else {
        panic!("const statement");
    };
    assert_eq!(name, "local");
    assert_site(source, *name_span, "local");
    assert_within(statements[0].span(), *name_span, "local const binder");

    let Statement::Var {
        name,
        name_span,
        keys,
        ty,
        ..
    } = &statements[1]
    else {
        panic!("var statement");
    };
    assert_eq!(name, "mutable");
    assert_site(source, *name_span, "mutable");
    assert_within(statements[1].span(), *name_span, "local var binder");
    let [slot] = keys.as_slice() else {
        panic!("expected one local variable key");
    };
    assert_site(source, slot.name_span, "slot");
    assert_name_type(source, &slot.ty, "int", &["int"]);
    assert_name_type(
        source,
        ty.as_ref().expect("variable type"),
        "bytes",
        &["bytes"],
    );

    let Statement::IfConst {
        name, name_span, ..
    } = &statements[2]
    else {
        panic!("if const statement");
    };
    assert_eq!(name, "present");
    assert_site(source, *name_span, "present");
    assert_within(statements[2].span(), *name_span, "if-const binder");

    let Statement::IfConstChain { bindings, .. } = &statements[3] else {
        panic!("if const chain");
    };
    let [first, second] = bindings.as_slice() else {
        panic!("expected two chained bindings");
    };
    assert_eq!(first.name, "first");
    assert_site(source, first.name_span, "first");
    assert_within(
        statements[3].span(),
        first.name_span,
        "first chained binder",
    );
    assert_eq!(second.name, "second");
    assert_site(source, second.name_span, "second");
    assert_within(
        statements[3].span(),
        second.name_span,
        "second chained binder",
    );

    let Statement::LetElse {
        name, name_span, ..
    } = &statements[4]
    else {
        panic!("let-else");
    };
    assert_eq!(name, "fallback");
    assert_site(source, *name_span, "fallback");
    assert_within(statements[4].span(), *name_span, "const let-else binder");

    let Statement::LetElse {
        is_var,
        name,
        name_span,
        ..
    } = &statements[5]
    else {
        panic!("var let-else");
    };
    assert!(*is_var);
    assert_eq!(name, "fallbackVar");
    assert_site(source, *name_span, "fallbackVar");
    assert_within(statements[5].span(), *name_span, "var let-else binder");

    let Statement::Checked { bind, .. } = &statements[6] else {
        panic!("checked const");
    };
    let CheckedBind::Const {
        name, name_span, ..
    } = bind
    else {
        panic!("checked const binding");
    };
    assert_eq!(name, "checkedConst");
    assert_site(source, *name_span, "checkedConst");
    assert_within(statements[6].span(), *name_span, "checked const binder");

    let Statement::Checked { bind, .. } = &statements[7] else {
        panic!("checked var");
    };
    let CheckedBind::Var {
        name, name_span, ..
    } = bind
    else {
        panic!("checked var binding");
    };
    assert_eq!(name, "checkedVar");
    assert_site(source, *name_span, "checkedVar");
    assert_within(statements[7].span(), *name_span, "checked var binder");

    let Statement::Expr {
        value:
            Expression::Call {
                args,
                span: call_span,
                ..
            },
        ..
    } = &statements[8]
    else {
        panic!("call statement");
    };
    let [positional, named] = args.as_slice() else {
        panic!("expected one positional and one named argument");
    };
    for argument in args {
        assert_eq!(
            argument.name.is_some(),
            argument.name_span.is_some(),
            "argument names and name spans must be parallel"
        );
    }
    assert_eq!(positional.name, None);
    assert_eq!(positional.name_span, None);
    assert_eq!(named.name.as_deref(), Some("named"));
    assert_site(
        source,
        named.name_span.expect("named argument span"),
        "named",
    );
    assert_within(
        *call_span,
        named.name_span.expect("named argument span"),
        "named argument",
    );
}

#[test]
fn malformed_nodes_retain_real_fallback_spans_and_remain_unavailable() {
    let source = concat!(
        "const : int = 1\n",
        "store broken\n",
        "fn broken() {\n",
        "    const = 1 else return\n",
        "    const = checked 1 + 2\n",
        "        on out_of_range {\n",
        "            return\n",
        "        }\n",
        "    var = checked 4 / 2\n",
        "        on zero_divisor {\n",
        "            return\n",
        "        }\n",
        "}\n",
    );
    let parsed = parse_source(source);
    assert!(
        parsed.has_errors(),
        "recovered syntax must stay unavailable to semantic processing"
    );

    let [
        Declaration::Const(top_const),
        Declaration::Store(store),
        Declaration::Function(function),
    ] = parsed.file.declarations.as_slice()
    else {
        panic!(
            "unexpected recovered declaration corpus: {:#?}",
            parsed.file.declarations
        );
    };
    assert_site(source, top_const.name_span, "const : int = 1");
    assert_within(
        top_const.span,
        top_const.name_span,
        "recovered top-level const",
    );
    assert_site(source, store.resource_span, "store broken");
    assert_within(store.span, store.resource_span, "recovered store resource");

    let [
        Statement::LetElse { name_span, .. },
        Statement::Checked {
            bind: const_bind, ..
        },
        Statement::Checked { bind: var_bind, .. },
    ] = function.body.statements.as_slice()
    else {
        panic!(
            "unexpected recovered statement corpus: {:#?}",
            function.body.statements
        );
    };
    assert_site(source, *name_span, "const");
    assert_within(
        function.body.statements[0].span(),
        *name_span,
        "recovered let-else binder",
    );
    let CheckedBind::Const { name_span, .. } = const_bind else {
        panic!("expected recovered checked const");
    };
    assert_site(source, *name_span, "=");
    assert_within(
        function.body.statements[1].span(),
        *name_span,
        "recovered checked const binder",
    );
    let CheckedBind::Var { name_span, .. } = var_bind else {
        panic!("expected recovered checked var");
    };
    assert_site(source, *name_span, "=");
    assert_within(
        function.body.statements[2].span(),
        *name_span,
        "recovered checked var binder",
    );
}

#[test]
fn canonical_formatting_is_byte_identical_and_idempotent() {
    let canonical = "const Limit: int = 7\n";
    assert_eq!(format_source(canonical), canonical);
    assert_eq!(format_source(&format_source(canonical)), canonical);
}
