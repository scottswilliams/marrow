//! `at most` bounds through the production path: a positive integer literal or a
//! module `const` of type `int` bounds a durable traversal; any other bound is refused
//! at the bound.

use crate::common::Project;

const SHELF_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A module `const` of type `int` bounds the walk exactly as the literal would, and
/// `on more` runs once a further key exists.
#[test]
fn a_module_const_bounds_a_durable_traversal() {
    let workspace = Project::single(
        r#"resource Book {
    required title: string
}

store ^books[id: int]: Book

const pageSize = 2

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn firstPage(): int {
    var seen: int = 0
    for id in ^books at most pageSize {
        seen += 1
    } on more {
        return seen + 100
    }
    return seen
}

test "a module const bounds the walk" {
    add(1, "a")
    add(2, "b")
    assert firstPage() == 2
    add(3, "c")
    assert firstPage() == 102
}
"#,
    )
    .ids(SHELF_IDS)
    .materialize("at-most-const");
    let output = workspace.marrow(&["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""failed":0"#), "{stdout}");
}

/// A bound that is not a positive `int` literal or module constant — a `string`
/// constant, a non-positive literal, a local binding — is a `check.type` at the bound.
#[test]
fn a_bound_that_is_not_a_positive_int_is_refused() {
    for (declaration, bound) in [("const limit = \"two\"", "limit"), ("", "0"), ("", "n")] {
        let diagnostics = Project::single(&format!(
            "resource Book {{\n    required title: string\n}}\n\n\
             store ^books[id: int]: Book\n\n\
             {declaration}\n\n\
             pub fn walk(): int {{\n\
             \x20   const n = 2\n\
             \x20   for id in ^books at most {bound} {{\n\
             \x20       return 1\n\
             \x20   }} on more {{\n\
             \x20       return 2\n\
             \x20   }}\n\
             \x20   return 0\n\
             }}\n"
        ))
        .ids(SHELF_IDS)
        .try_image()
        .expect_err("the bound is refused");
        let row = diagnostics.only("check.type");
        assert_eq!(
            (row.line(), row.column()),
            (11, 30),
            "{bound}: {:?}",
            diagnostics.all()
        );
    }
}

/// A named bound resolves as the reference states: a local resolves before a module
/// declaration of the same name, a module `const` of type `int` is admitted only when
/// positive and within the ceiling, and any other name is refused at the bound.
/// One named-bound case: the module declaration, the local statement preceding the
/// loop, the bound as written, and the `(code, line)` rows the compile reports.
struct NamedBound {
    label: &'static str,
    declaration: &'static str,
    local: &'static str,
    bound: &'static str,
    expected: Vec<(marrow_codes::Code, u32)>,
}

#[test]
fn a_named_bound_resolves_as_the_reference_states() {
    use marrow_codes::Code;
    let case = |label, declaration, local, bound, expected| NamedBound {
        label,
        declaration,
        local,
        bound,
        expected,
    };
    let cases = [
        case(
            "a folded expression",
            "const limit = 1 + 1",
            "const other = 2",
            "limit",
            vec![(Code::CheckUnsupported, 7), (Code::CheckUnsupported, 11)],
        ),
        case(
            "a named zero",
            "const zero = 0",
            "const other = 2",
            "zero",
            vec![(Code::CheckType, 11)],
        ),
        case(
            "a named negative",
            "const neg = -1",
            "const other = 2",
            "neg",
            vec![(Code::CheckType, 11)],
        ),
        case(
            "the exact ceiling",
            "const cap = 65536",
            "const other = 2",
            "cap",
            vec![],
        ),
        case(
            "one over the ceiling",
            "const over = 65537",
            "const other = 2",
            "over",
            vec![(Code::CheckType, 11)],
        ),
        case(
            "a const of another type",
            "const label = \"two\"",
            "const other = 2",
            "label",
            vec![(Code::CheckType, 11)],
        ),
        case(
            "a local shadowing the module const",
            "const n = 3",
            "const n = 2",
            "n",
            vec![(Code::CheckType, 11)],
        ),
    ];
    for NamedBound {
        label,
        declaration,
        local,
        bound,
        expected,
    } in cases
    {
        let outcome = Project::single(&format!(
            "resource Book {{\n    required title: string\n}}\n\n\
             store ^books[id: int]: Book\n\n\
             {declaration}\n\n\
             pub fn walk(): int {{\n\
             \x20   {local}\n\
             \x20   for id in ^books at most {bound} {{\n\
             \x20       return 1\n\
             \x20   }} on more {{\n\
             \x20       return 2\n\
             \x20   }}\n\
             \x20   return 0\n\
             }}\n"
        ))
        .ids(SHELF_IDS)
        .try_image();
        let actual: Vec<(Code, u32)> = match &outcome {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .iter()
                .map(|row| (row.code(), row.line()))
                .collect(),
        };
        assert_eq!(actual, expected, "{label}");
    }
}
