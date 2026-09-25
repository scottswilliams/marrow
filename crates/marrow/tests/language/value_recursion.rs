//! The type-recursion rule of `docs/language/types-and-values.md#recursive-types`,
//! pinned form by form through capture, compile, and independent verification.
//!
//! A struct or enum may hold its own type, at the same type arguments, only inside a
//! `List` or `Map`. Every refused row asserts its exact `(code, line, column)` multiset;
//! every admitted row must reach a verified image, so a verifier that disagreed with the
//! checker would fail here rather than at a runner.
//!
//! The module also pins the runtime charge the same reference section states: a
//! subtree appended to a worklist counts in full toward the collection bound.
//!
//! The cycle path of a `check.recursion` is read from the message suffix: no typed fact
//! carries it yet, and the error-code registry promises that the message names the
//! cycle. That is this module's one exception to the harness's no-prose rule.

use crate::common::{CallOutcome, Diagnostics, Project};
use marrow_codes::Code;
use marrow_vm::Value;

/// One expected diagnostic: its code, line, column, and for `check.recursion` the cycle
/// path the message names.
type Row = (&'static str, u32, u32, Option<&'static str>);

const RECURSION: &str = "check.recursion";

/// The diagnostics as sorted `(code, line, column, cycle)` rows.
fn located(diagnostics: &Diagnostics) -> Vec<(String, u32, u32, Option<String>)> {
    let mut rows: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            let code = d.code().as_str();
            let cycle = (code == RECURSION).then(|| {
                let (_, path) = d
                    .message()
                    .split_once("through the cycle ")
                    .unwrap_or_else(|| panic!("a recursion names its cycle: {}", d.message()));
                path.to_string()
            });
            (code.to_string(), d.line(), d.column(), cycle)
        })
        .collect();
    rows.sort();
    rows
}

fn assert_refused(label: &str, source: &str, expected: &[Row]) {
    let Err(diagnostics) = Project::single(source).try_image() else {
        panic!("{label}: the program must be refused");
    };
    let mut expected: Vec<_> = expected
        .iter()
        .map(|&(code, line, column, cycle)| {
            (code.to_string(), line, column, cycle.map(str::to_string))
        })
        .collect();
    expected.sort();
    assert_eq!(located(&diagnostics), expected, "{label}");
}

fn assert_admitted(label: &str, source: &str) {
    if let Err(diagnostics) = Project::single(source).try_image() {
        panic!("{label}: must verify, got {:?}", diagnostics.all());
    }
}

#[test]
fn self_containing_types_are_refused_at_their_sites() {
    let cases: &[(&str, &str, &[Row])] = &[
        (
            "a field of its own type",
            "struct Node {\n    next: Node\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("Node -> Node"))],
        ),
        (
            "a cycle through another struct",
            "struct A {\n    b: B\n}\n\nstruct B {\n    a: A\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[
                (RECURSION, 1, 8, Some("A -> B -> A")),
                (RECURSION, 5, 8, Some("B -> A -> B")),
            ],
        ),
        (
            "an enum payload of its own type",
            "enum E {\n    leaf\n    node(v: E)\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 6, Some("E -> E"))],
        ),
        (
            "an enum through a struct payload",
            "enum E {\n    leaf\n    node(s: S)\n}\n\nstruct S {\n    e: E\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[
                (RECURSION, 1, 6, Some("E -> S -> E")),
                (RECURSION, 6, 8, Some("S -> E -> S")),
            ],
        ),
        (
            "through Option",
            "struct A {\n    v: int\n    me: Option<A>\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("A -> Option<A> -> A"))],
        ),
        (
            "through the ok side of Result",
            "struct A {\n    v: int\n    r: Result<A, string>\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("A -> Result<A, string> -> A"))],
        ),
        (
            "through the error side of Result",
            "struct A {\n    r: Result<int, A>\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("A -> Result<int, A> -> A"))],
        ),
        (
            "a generic struct applied to the same argument",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nfn useIt(b: Box<int>): int {\n    return b.v\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("Box<int> -> Box<int>"))],
        ),
        (
            "a generic struct applied in a generic function that is called",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nfn g<U>(x: U): int {\n    const xs: List<Box<int>> = List()\n    return length(xs)\n}\n\npub fn f(): int {\n    return g(1)\n}\n",
            &[(RECURSION, 1, 8, Some("Box<int> -> Box<int>"))],
        ),
        (
            "a generic struct applied in a generic function reached through another",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nfn g<U>(x: U): int {\n    const xs: List<Box<int>> = List()\n    return length(xs)\n}\n\nfn h<V>(y: V): int {\n    return g(1)\n}\n\npub fn f(): int {\n    return h(true)\n}\n",
            &[(RECURSION, 1, 8, Some("Box<int> -> Box<int>"))],
        ),
        (
            "a generic struct applied in the field of an applied generic type",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nstruct Other<U> {\n    u: U\n    b: List<Box<int>>\n}\n\nfn useIt(o: Other<int>): int {\n    return o.u\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 8, Some("Box<int> -> Box<int>"))],
        ),
        (
            "a generic enum applied to the same argument",
            "enum Tree<T> {\n    leaf(v: T)\n    node(l: Tree<T>, r: Tree<T>)\n}\n\nfn useIt(t: Tree<int>): int {\n    return 0\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(RECURSION, 1, 6, Some("Tree<int> -> Tree<int>"))],
        ),
        (
            "a generic struct through Option",
            "struct Node<T> {\n    v: T\n    next: Option<Node<T>>\n}\n\nfn useIt(n: Node<int>): int {\n    return n.v\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[(
                RECURSION,
                1,
                8,
                Some("Node<int> -> Option<Node<int>> -> Node<int>"),
            )],
        ),
        (
            // `Wrap<T>` alone is not recursive but is also reported through the `A`
            // cycle; a checker fix that drops the 1:8 row is intended.
            "a struct through a generic argument",
            "struct Wrap<T> {\n    inner: T\n}\n\nstruct A {\n    w: Wrap<A>\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[
                (RECURSION, 1, 8, Some("Wrap<A> -> A -> Wrap<A>")),
                (RECURSION, 5, 8, Some("A -> Wrap<A> -> A")),
            ],
        ),
    ];
    for (label, source, expected) in cases {
        assert_refused(label, source, expected);
    }
}

#[test]
fn an_application_holding_itself_at_a_growing_argument_reaches_the_instantiation_limit() {
    let cases: &[(&str, &str, &[Row])] = &[
        (
            "an application of a type holding itself at a growing argument inside a list",
            "struct Node<T> {\n    value: T\n    kids: List<Node<List<T>>>\n}\n\nfn useIt(n: Node<int>): int {\n    return n.value\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 6, 13, None)],
        ),
        (
            "a growing application in the body of a generic function that is never called, \
             reported at the enclosing written type",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\nfn g<U>(x: U): int {\n    const xs: List<Box<int>> = List()\n    return length(xs)\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 6, 15, None)],
        ),
        (
            "a growing application in the signature of a generic function that is never called",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\nfn g<U>(x: U, bs: List<Box<int>>): int {\n    return length(bs)\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 5, 19, None)],
        ),
        (
            "a growing application in the field of an applied template, reported at the \
             template's application",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\nstruct Other<U> {\n    u: U\n    b: List<Box<int>>\n}\n\nfn useIt(o: Other<int>): int {\n    return o.u\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 10, 13, None)],
        ),
    ];
    for (label, source, expected) in cases {
        assert_refused(label, source, expected);
    }
}

#[test]
fn an_enum_payload_cannot_be_a_collection_even_of_the_enum() {
    assert_refused(
        "a list of the enum as a payload",
        "enum E {\n    leaf\n    node(kids: List<E>)\n}\n\npub fn f(): int {\n    return 1\n}\n",
        &[("check.unsupported", 3, 16, None)],
    );
}

#[test]
fn a_list_or_map_ends_a_type_cycle() {
    for (label, source) in [
        (
            "a struct holding a list of itself",
            "struct Tree {\n    v: int\n    kids: List<Tree>\n}\n\npub fn f(): int {\n    const leaf = Tree(v: 2, kids: List())\n    const root = Tree(v: 1, kids: List(leaf))\n    return length(root.kids)\n}\n",
        ),
        (
            "a struct holding a map of itself",
            "struct Tree {\n    v: int\n    kids: Map<string, Tree>\n}\n\npub fn f(): int {\n    var kids: Map<string, Tree> = Map()\n    kids[\"a\"] = Tree(v: 2, kids: Map())\n    const root = Tree(v: 1, kids: kids)\n    return root.v\n}\n",
        ),
        (
            "a list of optional selves",
            "struct A {\n    v: int\n    kids: List<Option<A>>\n}\n\npub fn f(): int {\n    const a = A(v: 1, kids: List())\n    return a.v\n}\n",
        ),
        (
            "a map of optional selves",
            "struct A {\n    v: int\n    m: Map<string, Option<A>>\n}\n\npub fn f(): int {\n    const a = A(v: 1, m: Map())\n    return a.v\n}\n",
        ),
        (
            "a list of lists of selves",
            "struct A {\n    v: int\n    grid: List<List<A>>\n}\n\npub fn f(): int {\n    const a = A(v: 1, grid: List())\n    return a.v\n}\n",
        ),
        (
            "a generic struct holding a list of its own application",
            "struct Node<T> {\n    value: T\n    kids: List<Node<T>>\n}\n\npub fn f(): int {\n    var kids: List<Node<int>> = List()\n    const n = Node(value: 1, kids: kids)\n    return n.value\n}\n",
        ),
        (
            "an enum through a struct payload holding a list of the enum",
            "enum Expr {\n    num(v: int)\n    sum(args: Args)\n}\n\nstruct Args {\n    items: List<Expr>\n}\n\npub fn f(): int {\n    const e = Expr::sum(args: Args(items: List(Expr::num(v: 2))))\n    return 1\n}\n",
        ),
        (
            "an enum through an optional struct payload holding a list of the enum",
            "enum E {\n    leaf\n    node(o: Option<Args>)\n}\n\nstruct Args {\n    items: List<E>\n}\n\npub fn f(): int {\n    const e = E::node(o: some(Args(items: List(E::leaf))))\n    return 1\n}\n",
        ),
        (
            "an enum through a result payload holding a list of the enum",
            "enum E {\n    leaf\n    node(r: Result<Args, int>)\n}\n\nstruct Args {\n    items: List<E>\n}\n\npub fn f(): int {\n    const e = E::node(r: ok(Args(items: List(E::leaf))))\n    return 1\n}\n",
        ),
        (
            "two structs whose cycle passes through a list",
            "struct A {\n    bs: List<B>\n}\n\nstruct B {\n    a: A\n}\n\npub fn f(): int {\n    const b = B(a: A(bs: List()))\n    return length(b.a.bs)\n}\n",
        ),
        (
            "a generic argument that is a list of the struct",
            "struct Wrap<T> {\n    inner: T\n}\n\nstruct A {\n    v: int\n    w: Wrap<List<A>>\n}\n\npub fn f(): int {\n    const empty: List<A> = List()\n    const a = A(v: 1, w: Wrap(inner: empty))\n    return a.v\n}\n",
        ),
        (
            "a resource field of a list-recursive struct, as a local value",
            "struct Tree {\n    v: int\n    kids: List<Tree>\n}\n\nresource R {\n    t: Tree\n}\n\npub fn f(): int {\n    const r = R(t: Tree(v: 1, kids: List(Tree(v: 2, kids: List()))))\n    const t = r.t else {\n        return 0\n    }\n    return length(t.kids)\n}\n",
        ),
        (
            "an export taking and returning a list-recursive struct",
            "struct Tree {\n    v: int\n    kids: List<Tree>\n}\n\npub fn f(root: Tree): Tree {\n    return root\n}\n",
        ),
    ] {
        assert_admitted(label, source);
    }
}

/// Recursion is checked for each application reached from non-generic code, directly
/// or through the generic functions it calls and the generic types it applies; a
/// template never applied from there is not checked for recursion, at the same
/// argument or a growing one. A growing application in the field of a template that is
/// never applied does not reach the instantiation limit either.
#[test]
fn a_template_not_applied_from_checked_code_is_not_checked_for_recursion() {
    for (label, source) in [
        (
            "an application only in a generic function that is never called",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nfn g<U>(x: U): int {\n    const xs: List<Box<int>> = List()\n    return length(xs)\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
        (
            "an application only in a generic function called only from an uncalled one",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\nfn g<U>(x: U): int {\n    const xs: List<Box<int>> = List()\n    return length(xs)\n}\n\nfn h<V>(y: V): int {\n    return g(1)\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
        (
            "a growing application only in the field of a template that is never applied",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\nstruct Other<U> {\n    b: List<Box<int>>\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
        (
            "an unapplied template holding itself",
            "struct Box<T> {\n    v: T\n    child: Box<T>\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
        (
            "an unapplied template holding itself at a growing argument in a list",
            "struct Node<T> {\n    value: T\n    kids: List<Node<List<T>>>\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
        (
            "an unapplied template holding itself at a growing argument",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\npub fn f(): int {\n    return 1\n}\n",
        ),
    ] {
        assert_admitted(label, source);
    }
}

/// A struct appended to a list is charged its full structural size, including every
/// list it holds. `inner` holds 20 leaves of 32 KiB labels, about 640 KiB, so one copy
/// fits under the 1 MiB collection bound and a second does not. Charging a held list
/// as its framing alone would admit both.
#[test]
fn an_appended_subtree_counts_in_full_toward_the_collection_bound() {
    let mut session = Project::single(
        "struct Tree {
    label: string
    kids: List<Tree>
}

pub fn pendingOf(copies: int): int {
    var label: string = \"x\"
    var doublings: int = 0
    while doublings < 15 {
        label += label
        doublings += 1
    }
    var leaves: List<Tree> = List()
    while length(leaves) < 20 {
        leaves = append(leaves, Tree(label: label, kids: List()))
    }
    const inner = Tree(label: \"\", kids: leaves)
    var pending: List<Tree> = List()
    while length(pending) < copies {
        pending = append(pending, inner)
    }
    return length(pending)
}
",
    )
    .session();
    assert_eq!(
        session.try_call("pendingOf", vec![Value::Int(1)]),
        CallOutcome::Value(Some(Value::Int(1)))
    );
    assert_eq!(
        session.try_call("pendingOf", vec![Value::Int(2)]),
        CallOutcome::Fault {
            code: Code::RunCollectionLimit,
            line: 20,
            column: 19,
        }
    );
}
