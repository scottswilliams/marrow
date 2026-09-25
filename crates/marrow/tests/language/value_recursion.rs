//! The type-recursion rule of `docs/language/types-and-values.md#recursive-types`,
//! pinned form by form through capture, compile, and independent verification.
//!
//! A struct or enum may hold its own type, at the same type arguments, only inside a
//! `List` or `Map`. Every refused row asserts its exact `(code, line, column)` multiset;
//! every admitted row must reach a verified image, so a verifier that disagreed with the
//! checker would fail here rather than at a runner.
//!
//! The cycle path of a `check.recursion` is read from the message suffix: no typed fact
//! carries it yet, and the error-code registry promises that the message names the
//! cycle. That is this module's one exception to the harness's no-prose rule.

use crate::common::{Diagnostics, Project};

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

#[test]
fn a_type_containing_itself_other_than_through_a_collection_is_refused() {
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
            // Current behavior: `Wrap` is also reported (forward plan F1); a fix that
            // drops the 1:8 row is intended.
            "a struct through a generic argument",
            "struct Wrap<T> {\n    inner: T\n}\n\nstruct A {\n    w: Wrap<A>\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[
                (RECURSION, 1, 8, Some("Wrap<A> -> A -> Wrap<A>")),
                (RECURSION, 5, 8, Some("A -> Wrap<A> -> A")),
            ],
        ),
        (
            "an application of a type holding itself at a growing argument inside a list",
            "struct Node<T> {\n    value: T\n    kids: List<Node<List<T>>>\n}\n\nfn useIt(n: Node<int>): int {\n    return n.value\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 6, 13, None)],
        ),
        (
            "an application of a type holding itself at a growing argument",
            "struct Box<T> {\n    child: Box<Option<T>>\n}\n\nfn useIt(b: Box<int>): int {\n    return 1\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.instantiation_limit", 5, 13, None)],
        ),
        (
            "an enum payload cannot be a collection, even of the enum",
            "enum E {\n    leaf\n    node(kids: List<E>)\n}\n\npub fn f(): int {\n    return 1\n}\n",
            &[("check.unsupported", 3, 16, None)],
        ),
    ];
    for (label, source, expected) in cases {
        assert_refused(label, source, expected);
    }
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
        // A template is checked per application, so an unapplied self-reference is
        // admitted, at the same argument or a growing one.
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
        if let Err(diagnostics) = Project::single(source).try_image() {
            panic!("{label}: must verify, got {:?}", diagnostics.all());
        }
    }
}

/// A collection is a local value: a resource field cannot be one.
#[test]
fn a_resource_field_cannot_be_a_list_or_map() {
    assert_refused(
        "a resource field of list type",
        "resource R {\n    l: List<int>\n}\n\npub fn f(): int {\n    return 1\n}\n",
        &[("check.unsupported", 2, 8, None)],
    );
}
