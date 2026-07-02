//! Static-soundness rejections: programs that once checked clean and then faulted at
//! runtime now fail check. Builtin arity, module-constant writability, undeclared
//! saved roots, call-keyed saved-read presence, and the local ban on optional
//! stored-shape positions all reject statically through the production checker.

use crate::support;

use support::{check_module, check_module_report, with_code};

const STORE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   pages: int\n\
     \x20   scores(pos: int): int\n\
     store ^books(id: int): Book\n";

fn assert_code(name: &str, src: &str, code: &str) {
    let found = check_module(name, src, code);
    assert!(!found.is_empty(), "expected {code}: {:#?}", found);
}

fn assert_clean(name: &str, src: &str) {
    let report = check_module_report(name, src);
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// Builtin arity: every fixed-arity builtin is arity-checked at compile time.

#[test]
fn print_with_no_argument_is_an_arity_error() {
    assert_code(
        "print-zero",
        "module m\nfn f()\n    print()\n",
        "check.call_argument",
    );
}

#[test]
fn print_with_extra_arguments_is_an_arity_error() {
    assert_code(
        "print-two",
        "module m\nfn f()\n    print(\"a\", \"b\")\n",
        "check.call_argument",
    );
}

#[test]
fn count_exists_next_id_with_no_argument_are_arity_errors() {
    for (name, call) in [
        ("count-zero", "var c = count()"),
        ("exists-zero", "if exists()\n        print(1)"),
        ("next-id-zero", "var v = nextId()"),
    ] {
        assert_code(
            name,
            &format!("{STORE}fn f()\n    {call}\n"),
            "check.call_argument",
        );
    }
}

#[test]
fn a_builtin_at_its_declared_arity_is_not_an_arity_error() {
    let report = check_module_report(
        "arity-ok",
        &format!("{STORE}fn f()\n    print(count(^books))\n"),
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

// Module-constant writability: a module `const` is immutable in a function body.

#[test]
fn a_compound_assignment_to_a_module_constant_is_rejected() {
    assert_code(
        "module-const-compound",
        "module m\nconst rate: int = 5\nfn f()\n    rate += 1\n",
        "check.invalid_assign_target",
    );
}

#[test]
fn a_plain_assignment_to_a_module_constant_is_rejected() {
    assert_code(
        "module-const-plain",
        "module m\nconst rate: int = 5\nfn f()\n    rate = 6\n",
        "check.invalid_assign_target",
    );
}

#[test]
fn a_local_var_shadowing_a_module_constant_stays_writable() {
    assert_clean(
        "module-const-shadow",
        "module m\nconst rate: int = 5\nfn f()\n    var rate: int = 1\n    rate += 1\n",
    );
}

// Undeclared saved roots reject statically wherever a `^root` is spelled.

#[test]
fn an_undeclared_root_write_is_rejected() {
    assert_code(
        "root-write",
        &format!("{STORE}fn f()\n    ^shelves(1).x = 1\n"),
        "check.unknown_root",
    );
}

#[test]
fn an_undeclared_root_is_rejected_in_every_position() {
    for (name, body) in [
        ("root-read", "var v = ^shelves(1).x ?? 0"),
        ("root-for", "for x in ^shelves\n        print(1)"),
        ("root-count", "var c = count(^shelves)"),
        ("root-next-id", "var v = nextId(^shelves)"),
        ("root-delete", "delete ^shelves(1)"),
    ] {
        assert_code(
            name,
            &format!("{STORE}fn f()\n    {body}\n"),
            "check.unknown_root",
        );
    }
}

#[test]
fn a_declared_root_is_not_an_unknown_root() {
    let report = check_module_report(
        "root-ok",
        &format!("{STORE}fn f()\n    var v = ^books(1).pages ?? 0\n"),
    );
    assert!(
        with_code(&report, "check.unknown_root").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

// A compound assignment resolves its saved target once, so a resolution error at the
// target is reported once, exactly as a plain assignment reports it.

#[test]
fn a_compound_assignment_reports_a_bad_field_once() {
    let report = check_module_report(
        "compound-once",
        &format!("{STORE}fn f()\n    ^books(1).nope += 1\n"),
    );
    assert_eq!(
        with_code(&report, "check.unknown_field").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

// Call-keyed saved reads join the maybe-present model: a bare read and a proof-less
// compound assignment reject, while a guard refuses to run the key's effect.

const CALL_KEY: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   scores(pos: int): int\n\
     store ^books(id: int): Book\n\
     fn k(): int\n\
     \x20   return 1\n";

#[test]
fn a_bare_call_keyed_saved_read_must_be_resolved() {
    assert_code(
        "call-key-bare",
        &format!("{CALL_KEY}fn f()\n    var v = ^books(1).scores(k())\n    print(v)\n"),
        "check.unresolved_optional",
    );
}

#[test]
fn a_proofless_call_keyed_compound_assignment_is_rejected() {
    assert_code(
        "call-key-compound",
        &format!("{CALL_KEY}fn f()\n    ^books(1).title = \"t\"\n    ^books(1).scores(k()) += 5\n"),
        "check.unresolved_optional",
    );
}

#[test]
fn a_coalesce_over_a_call_keyed_saved_read_refuses_the_effect() {
    assert_code(
        "call-key-coalesce",
        &format!("{CALL_KEY}fn f()\n    print(^books(1).scores(k()) ?? -1)\n"),
        "check.operator_type",
    );
}

#[test]
fn an_if_const_over_a_call_keyed_saved_read_refuses_the_effect() {
    assert_code(
        "call-key-if-const",
        &format!("{CALL_KEY}fn f()\n    if const v = ^books(1).scores(k())\n        print(v)\n"),
        "check.condition_type",
    );
}

#[test]
fn an_exists_over_a_call_keyed_saved_read_refuses_the_effect() {
    assert_code(
        "call-key-exists",
        &format!("{CALL_KEY}fn f()\n    if exists(^books(1).scores(k()))\n        print(1)\n"),
        "check.call_argument",
    );
}

#[test]
fn a_hoisted_key_makes_the_saved_read_guardable() {
    assert_clean(
        "call-key-hoisted",
        &format!("{CALL_KEY}fn f()\n    const key = k()\n    print(^books(1).scores(key) ?? -1)\n"),
    );
}

// Local optional stored-shape positions: `?` on a keyed leaf or a sequence element is
// rejected the same way whether the tree is local or saved.

#[test]
fn a_local_keyed_leaf_may_not_be_optional() {
    assert_code(
        "local-keyed-leaf",
        "module m\nfn f()\n    var counts(name: string): int?\n    print(1)\n",
        "schema.optional_in_saved",
    );
}

#[test]
fn a_local_sequence_element_may_not_be_optional() {
    assert_code(
        "local-seq-element",
        "module m\nfn f()\n    var xs: sequence[string?]\n    print(1)\n",
        "schema.optional_in_saved",
    );
}

#[test]
fn a_keyed_parameter_leaf_may_not_be_optional() {
    assert_code(
        "param-keyed-leaf",
        "module m\nfn f(counts(name: string): int?)\n    print(1)\n",
        "schema.optional_in_saved",
    );
}

#[test]
fn an_optional_sequence_return_element_is_rejected() {
    assert_code(
        "return-seq-element",
        "module m\nfn f(): sequence[int?]\n    var xs: sequence[int]\n    return xs\n",
        "schema.optional_in_saved",
    );
}

#[test]
fn a_plain_local_optional_binding_is_allowed() {
    assert_clean(
        "local-scalar-optional",
        "module m\nfn f(note: string?): int?\n    var v: int? = absent\n    return v\n",
    );
}
