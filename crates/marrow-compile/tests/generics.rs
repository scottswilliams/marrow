//! Rank-1 generic function checking and monomorphization through the production
//! `compile` path: type-argument inference, the once-checked template pass against
//! `supports equality`/`supports order` constraints, per-application revalidation,
//! the instantiation bound, and the image-local (no stable identity) nature of
//! monomorphized instances.

use marrow_compile::{Compiled, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// Capture a single-module project from source, the way the CLI adapter feeds the
/// compiler, so these tests exercise the real capture + compile path.
fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn compile_ok(source: &str) -> Compiled {
    compile(&project(source)).unwrap_or_else(|diagnostics| {
        panic!("expected a clean compile, got {diagnostics:#?}");
    })
}

fn compile_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the program compiled"),
        Err(diagnostics) => diagnostics,
    }
}

fn has_code(diagnostics: &[SourceDiagnostic], code: &str) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

/// A public generic function and every one of its monomorphized instances mint no
/// export (stable hash identity): only public monomorphic functions appear in the
/// export directory, however many times the generic is instantiated.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn generic_instances_mint_no_stable_identity() {
    let compiled = compile_ok(
        "module main\n\
         \n\
         pub fn identity[T](x: T): T\n\
         \x20   return x\n\
         \n\
         pub fn driver(): int\n\
         \x20   const a = identity(1)\n\
         \x20   const b = identity(\"two\")\n\
         \x20   const c = identity(true)\n\
         \x20   const d = identity(a)\n\
         \x20   return a\n",
    );
    // Four distinct instantiations of `identity` were minted (int, string, bool,
    // and a second int reusing the first), yet the only export is the monomorphic
    // `driver`; neither `identity` nor any instance has a stable identity.
    let export_items: Vec<&str> = compiled
        .exports
        .iter()
        .map(|export| export.item.as_str())
        .collect();
    assert_eq!(
        export_items,
        ["driver"],
        "only the concrete public fn exports"
    );
}

/// A type parameter that no argument determines cannot be inferred (there is no
/// explicit instantiation syntax), and the call is a typed `check.type`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_type_parameter_no_argument_determines_cannot_be_inferred() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn make[T](): T?\n\
         \x20   return absent\n\
         \n\
         pub fn driver(): int\n\
         \x20   const x = make()\n\
         \x20   return 0\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("cannot infer type parameter `T`")),
        "{diagnostics:#?}"
    );
}

/// The once-checked template pass rejects `==` over an unconstrained type
/// parameter, independently of whether the generic is ever instantiated.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn equality_on_an_unconstrained_parameter_is_rejected_in_the_body() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn same[T](a: T, b: T): bool\n\
         \x20   return a == b\n\
         \n\
         pub fn driver(): int\n\
         \x20   return 0\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("supports equality")),
        "{diagnostics:#?}"
    );
}

/// The once-checked template pass rejects `<` over a parameter constrained only by
/// equality: order is a distinct constraint.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn order_on_an_equality_only_parameter_is_rejected_in_the_body() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn smaller[T supports equality](a: T, b: T): bool\n\
         \x20   return a < b\n\
         \n\
         pub fn driver(): int\n\
         \x20   return 0\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("supports order")),
        "{diagnostics:#?}"
    );
}

/// A constrained generic body checks, but a call that instantiates the parameter
/// with a concrete type that does not support the constraint is revalidated and
/// rejected at the call site.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_call_revalidates_the_constraint_against_the_concrete_type() {
    // `bool` supports equality but not order: instantiating an order-constrained
    // parameter with `bool` is rejected per application.
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn smaller[T supports order](a: T, b: T): bool\n\
         \x20   return a < b\n\
         \n\
         pub fn driver(): bool\n\
         \x20   return smaller(true, false)\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("does not `supports order`")),
        "{diagnostics:#?}"
    );
}

/// A generic body may call a monomorphic function that takes a concrete
/// collection type: the once-checked template pass sees the concrete callee's
/// collection type at the same index the real image records it, so the call is not
/// falsely rejected.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_body_calls_a_concrete_collection_typed_function() {
    compile_ok(
        "module main\n\
         \n\
         fn total(xs: List[int]): int\n\
         \x20   var sum: int = 0\n\
         \x20   for x in xs\n\
         \x20       sum = sum + x\n\
         \x20   return sum\n\
         \n\
         fn wrap[T](x: T): int\n\
         \x20   var ns: List[int] = List()\n\
         \x20   ns = append(ns, 1)\n\
         \x20   ns = append(ns, 2)\n\
         \x20   return total(ns)\n\
         \n\
         pub fn driver(): int\n\
         \x20   return wrap(true)\n",
    );
}

/// The same generic instantiated at two different concrete types both check: the
/// body is checked once against the constraint, and each application revalidates.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_constrained_generic_instantiates_at_several_supporting_types() {
    compile_ok(
        "module main\n\
         \n\
         fn smaller[T supports order](a: T, b: T): bool\n\
         \x20   return a < b\n\
         \n\
         pub fn driver(): bool\n\
         \x20   const byInt = smaller(1, 2)\n\
         \x20   const byText = smaller(\"a\", \"b\")\n\
         \x20   return byInt\n",
    );
}

/// A generic self-call at the same instantiation is a recursion cycle over the
/// instance's image function, rejected as `check.recursion`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_recursing_at_the_same_instantiation_is_recursion() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn spin[T](x: T): T\n\
         \x20   return spin(x)\n\
         \n\
         pub fn driver(): int\n\
         \x20   return spin(1)\n",
    );
    assert!(
        has_code(&diagnostics, "check.recursion"),
        "{diagnostics:#?}"
    );
}

/// A generic that recurses over an ever-growing type diverges monomorphization;
/// the instantiation bound (law 9) fails it with a typed `check.instantiation_limit`
/// rather than looping unboundedly.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn divergent_monomorphization_hits_the_instantiation_bound() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         fn grow[T](x: T): int\n\
         \x20   var xs: List[T] = List()\n\
         \x20   xs = append(xs, x)\n\
         \x20   return grow(xs)\n\
         \n\
         pub fn driver(): int\n\
         \x20   return grow(1)\n",
    );
    assert!(
        has_code(&diagnostics, "check.instantiation_limit"),
        "{diagnostics:#?}"
    );
}

// --- user-definable generic value types (slice 3) ---

/// A generic `struct` and `enum` are templates, not concrete image types: they mint
/// nothing until used, and neither a template nor any of its instantiations is a
/// stable export. The only exports are the monomorphic `pub` functions.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn generic_type_instantiations_mint_no_stable_identity() {
    let compiled = compile_ok(
        "module main\n\
         \n\
         struct Pair[A, B]\n\
         \x20   first: A\n\
         \x20   second: B\n\
         \n\
         enum Box[T]\n\
         \x20   empty\n\
         \x20   full(value: T)\n\
         \n\
         pub fn run(): int\n\
         \x20   const p = Pair(first: 1, second: \"x\")\n\
         \x20   const q = Pair(first: true, second: 2)\n\
         \x20   const b = Box::full(value: 9)\n\
         \x20   return p.first\n",
    );
    let exports: Vec<&str> = compiled.exports.iter().map(|e| e.item.as_str()).collect();
    assert_eq!(exports, vec!["run"]);
}

/// A generic struct field is read at the concrete substituted type; a wrong field
/// name is a typed error against the instantiation, not a panic.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_struct_field_is_typed_by_its_instantiation() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         struct Wrapper[T]\n\
         \x20   value: T\n\
         \n\
         pub fn run(): int\n\
         \x20   const w = Wrapper(value: 3)\n\
         \x20   return w.missing\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
}

/// A generic type's `supports order` constraint is revalidated at construction: an
/// argument that does not support ordering is rejected.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_type_constraint_is_revalidated_at_construction() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         struct Ordered[T supports order]\n\
         \x20   lo: T\n\
         \x20   hi: T\n\
         \n\
         struct Point\n\
         \x20   x: int\n\
         \n\
         pub fn run(): int\n\
         \x20   const o = Ordered(lo: Point(x: 1), hi: Point(x: 2))\n\
         \x20   return 0\n",
    );
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
}

/// A monomorphized generic type cycle (`Tree[int]` directly containing `Tree[int]`)
/// is an ordinary value cycle per instantiation and is rejected as recursion at the
/// template's declaration.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_type_containing_itself_is_a_value_cycle() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         struct Tree[T]\n\
         \x20   value: T\n\
         \x20   child: Tree[T]\n\
         \n\
         fn useTree(t: Tree[int]): int\n\
         \x20   return t.value\n\
         \n\
         pub fn run(): int\n\
         \x20   return 0\n",
    );
    assert!(
        has_code(&diagnostics, "check.recursion"),
        "{diagnostics:#?}"
    );
}

/// A cycle broken by a collection (`struct Node[T]` whose field is `List[Node[T]]`)
/// is a finite value and is admitted: a list terminates, so it adds no containment
/// edge.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_generic_type_cycle_through_a_collection_is_admitted() {
    compile_ok(
        "module main\n\
         \n\
         struct Node[T]\n\
         \x20   value: T\n\
         \x20   kids: List[Node[T]]\n\
         \n\
         pub fn run(): int\n\
         \x20   var kids: List[Node[int]] = List()\n\
         \x20   const n = Node(value: 1, kids: kids)\n\
         \x20   return n.value\n",
    );
}

/// A generic type recursing over an ever-growing argument (`Grow[T]` whose field is
/// `Grow[List[T]]`) diverges under monomorphization and hits the shared
/// instantiation bound rather than looping.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_divergent_generic_type_hits_the_instantiation_bound() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         struct Grow[T]\n\
         \x20   value: T\n\
         \x20   next: Grow[List[T]]\n\
         \n\
         fn useGrow(g: Grow[int]): int\n\
         \x20   return g.value\n\
         \n\
         pub fn run(): int\n\
         \x20   return 0\n",
    );
    assert!(
        has_code(&diagnostics, "check.instantiation_limit"),
        "{diagnostics:#?}"
    );
}

/// `Option` and `Result` are ordinary generic enums the toolchain registers, not a
/// built-in special case: a user cannot redeclare their reserved names.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn the_reserved_generic_names_cannot_be_redeclared() {
    let diagnostics = compile_err(
        "module main\n\
         \n\
         enum Option[T]\n\
         \x20   nothing\n\
         \x20   something(value: T)\n\
         \n\
         pub fn run(): int\n\
         \x20   return 0\n",
    );
    assert!(
        has_code(&diagnostics, "check.name_conflict"),
        "{diagnostics:#?}"
    );
}

/// `Option[Option[int]]` is a distinct instantiation from `Option[int]`: the reserved
/// generic enum monomorphizes by argument exactly like a user generic enum.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn nested_option_is_a_distinct_instantiation() {
    compile_ok(
        "module main\n\
         \n\
         pub fn run(): int\n\
         \x20   const inner: Option[int] = some(1)\n\
         \x20   const outer: Option[Option[int]] = some(inner)\n\
         \x20   match outer\n\
         \x20       none\n\
         \x20           return 0\n\
         \x20       some(v)\n\
         \x20           match v\n\
         \x20               none\n\
         \x20                   return 0\n\
         \x20               some(k)\n\
         \x20                   return k\n",
    );
}
