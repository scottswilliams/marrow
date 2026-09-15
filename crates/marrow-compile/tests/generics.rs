//! Rank-1 generic function checking and monomorphization through the production
//! `compile` path: type-argument inference, the once-checked template pass against
//! `supports equality`/`supports order` constraints, per-application revalidation,
//! the instantiation bound, and the image-local (no stable identity) nature of
//! monomorphized instances.

use marrow_codes::Code;
use marrow_compile::compile_with_tests;
use marrow_compile::{CompileFailure, CompileInvariant, NonEmptySourceDiagnostics};
use marrow_compile::{Compiled, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use std::fmt::Write as _;

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

#[test]
fn nominal_boundaries_reject_a_public_record_input() {
    let source = "module main\ntype Age: int in 0..=150\nstruct Person { age: Age }\npub fn outside(p: Person): bool { return p.age > Age(150) }\n";
    assert_diagnostic_sites(&compile_err(source), &[(Code::CheckUnsupported, 4, 19)]);
}

#[test]
fn nominal_boundaries_follow_actual_public_value_leaves() {
    let cases = [
        ("struct Box<T> { value: T }", "Box<Age>"),
        ("enum Choice<T> { full(value: T) }", "Choice<Age>"),
        ("", "Option<Age>"),
        ("", "Result<Age, int>"),
        ("", "Result<int, Age>"),
        ("", "List<Age>"),
        ("", "Map<Age, int>"),
        ("", "Map<int, Age>"),
        ("struct Batch { ages: List<Age> }", "Option<Batch>"),
        ("resource R { required age: Age }", "R"),
        ("resource R { age: Age }", "R"),
        ("struct Person { age: Age }\nalias Saved = Person", "Saved"),
    ];
    for (declarations, parameter) in cases {
        let prelude = format!("module main\ntype Age: int in 0..=150\n{declarations}\n");
        let line = prelude.lines().count() as u32 + 1;
        let source = format!("{prelude}pub fn take(p: {parameter}): int {{ return 0 }}\n");
        assert_diagnostic_sites(&compile_err(&source), &[(Code::CheckUnsupported, line, 16)]);
    }
}

#[test]
fn nominal_boundaries_keep_optional_parameters_refused() {
    for parameter in ["Age?", "Person?", "Maybe"] {
        let source = format!(
            "module main\ntype Age: int in 0..=150\nstruct Person {{ age: Age }}\nalias Maybe = Age?\npub fn take(p: {parameter}): int {{ return 0 }}\n"
        );
        assert_diagnostic_sites(&compile_err(&source), &[(Code::CheckUnsupported, 5, 16)]);
    }
}

#[test]
fn nominal_boundaries_preserve_private_values_outputs_and_phantom_arguments() {
    compile_ok(
        "module main\ntype Age: int in 0..=150\nstruct Person { age: Age }\nstruct Phantom<T> { value: int }\nenum Flag<T> { yes }\nresource Local { required age: Age }\nfn private(p: Person): Person { return p }\npub fn bare(age: Age): Age { return age }\npub fn phantom(p: Phantom<Age>, flag: Flag<Age>): int { return p.value }\npub fn make(): Person { return private(Person(age: Age(150))) }\n",
    );
}

#[test]
fn nominal_boundaries_preserve_a_nominal_free_resource_with_generic_fields_and_groups() {
    compile_ok(
        "module main\nresource R {\n    reading: Option<int>\n    details { count: int }\n}\npub fn take(p: R): int { return 0 }\n",
    );
}

#[test]
fn nominal_boundaries_reject_nominals_inside_owned_groups() {
    let source = "module main\ntype Age: int in 0..=150\nresource R {\n    reading: Option<int>\n    details { age: Age }\n}\npub fn take(p: R): int { return 0 }\n";
    assert_diagnostic_sites(&compile_err(source), &[(Code::CheckUnsupported, 7, 16)]);
}

#[test]
fn nominal_boundaries_follow_collection_cycles_past_a_shared_node() {
    for (leaf, public) in [("age: Age", "A"), ("age: Age", "B"), ("age: int", "B")] {
        let source = format!(
            "module main\ntype Age: int in 0..=150\nstruct A {{\n    children: List<B>\n    {leaf}\n}}\nstruct B {{ parents: List<A> }}\npub fn take(p: {public}): int {{ return 0 }}\n"
        );
        if leaf == "age: int" {
            compile_ok(&source);
        } else {
            assert_diagnostic_sites(&compile_err(&source), &[(Code::CheckUnsupported, 8, 16)]);
        }
    }
}

#[test]
fn nominal_boundaries_have_no_durable_depth_cutoff_or_per_export_expansion() {
    let mut source =
        String::from("module main\ntype Age: int in 0..=150\nstruct S0 { age: Age }\n");
    for level in 1..=40 {
        writeln!(
            source,
            "struct S{level} {{\n    left: S{}\n    right: S{}\n}}",
            level - 1,
            level - 1
        )
        .expect("write source");
    }
    let first_line = source.lines().count() as u32 + 1;
    for export in 0..32 {
        writeln!(source, "pub fn take{export}(p: S40): int {{ return 0 }}").expect("write source");
    }
    // A single referring boundary is enough to withhold the whole image. The
    // shared graph has 41 identities but exponentially many occurrence paths.
    assert_diagnostic_sites(
        &compile_err(&source),
        &[(Code::CheckUnsupported, first_line, 17)],
    );
}

#[test]
fn nominal_boundaries_preserve_independent_body_diagnostics() {
    let diagnostics = compile_err(
        "module main\ntype Age: int in 0..=150\nstruct Person { age: Age }\npub fn take(p: Person): int { return 0 }\nfn bad(): int { return true }\n",
    );
    assert_diagnostic_sites(
        &diagnostics,
        &[(Code::CheckUnsupported, 4, 16), (Code::CheckType, 5, 24)],
    );
}

fn compile_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the program compiled"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(CompileFailure::ResourceLimit(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        Err(CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

fn compile_tests_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile_with_tests(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the project tests compiled"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(CompileFailure::ResourceLimit(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        Err(CompileFailure::Invariant(_)) => {
            panic!("source-triggered test compilation failures must remain diagnostics")
        }
    }
}

/// Capture and compile an exact set of module files so a diagnostic whose source
/// expression and requesting call live in different files cannot mix their locations.
fn compile_files_err(files: &[(&str, &str)]) -> Vec<SourceDiagnostic> {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = files
        .iter()
        .map(|(path, source)| CapturedFile::new((*path).to_string(), source.as_bytes().to_vec()))
        .collect();
    let project = marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture multi-file project");
    match compile(&project) {
        Ok(_) => panic!("expected a diagnostic, but the multi-file program compiled"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(CompileFailure::ResourceLimit(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        Err(CompileFailure::Invariant(_)) => {
            panic!("source-triggered multi-file compiler failures must remain diagnostics")
        }
    }
}

fn has_code(diagnostics: &[SourceDiagnostic], code: Code) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code() == code)
}

fn assert_one_located_limit(diagnostics: &[SourceDiagnostic], line: u32, column: u32) {
    assert_eq!(
        diagnostics.len(),
        1,
        "a limit refusal must not cascade: {diagnostics:#?}"
    );
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code(), Code::CheckInstantiationLimit);
    assert_eq!(diagnostic.file().as_str(), "src/main.mw");
    assert_eq!((diagnostic.line(), diagnostic.column()), (line, column));
}

fn assert_diagnostic_sites(diagnostics: &[SourceDiagnostic], expected: &[(Code, u32, u32)]) {
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file().as_str() == "src/main.mw"),
        "every diagnostic must retain the source file: {diagnostics:#?}"
    );
    let actual: Vec<(Code, u32, u32)> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code(), diagnostic.line(), diagnostic.column()))
        .collect();
    assert_eq!(actual, expected, "{diagnostics:#?}");
}

#[test]
fn alias_global_targets_cannot_capture_function_parameters() {
    for (parameter, return_column) in [("Item", 48), ("T", 42)] {
        let source = format!(
            "module main\nstruct Item {{ value: int }}\nalias SavedItem = Item\nfn identity<{parameter}>(x: {parameter}): SavedItem {{ return x }}\npub fn driver(): int {{ return identity(1) }}\n"
        );
        let diagnostics = compile_err(&source);
        assert_diagnostic_sites(
            &diagnostics,
            &[
                (Code::CheckType, 4, return_column),
                (Code::CheckType, 5, 31),
                (Code::CheckType, 4, return_column),
            ],
        );
    }
}

#[test]
fn written_function_parameters_shadow_aliases() {
    compile_ok(
        "module main\nalias T = int\nfn identity<T>(x: T): T { return x }\npub fn driver(): bool { return identity(true) }\n",
    );
}

/// The public failure boundary has exactly two externally matchable arms. Source
/// diagnostics retain their original ordered allocation behind the nonempty owner;
/// compiler-private causes remain payload-blind to external consumers.
#[test]
fn public_compile_failure_is_exhaustive_nonempty_and_worker_safe() {
    fn assert_error<T: std::error::Error>() {}
    fn assert_worker<T: Send + Sync + 'static>() {}
    fn assert_diagnostic_owner<T: std::fmt::Debug + Clone + PartialEq + Eq>() {}

    assert_error::<CompileFailure>();
    assert_error::<CompileInvariant>();
    assert_worker::<CompileInvariant>();
    assert_diagnostic_owner::<NonEmptySourceDiagnostics>();

    let failure = compile(&project(
        "module main\n\npub fn first(): int {\n    return true\n}\n\npub fn second(): int {\n    return false\n}\n",
    ))
    .expect_err("the two mismatched return types are source diagnostics");

    match failure {
        CompileFailure::Diagnostics(diagnostics) => {
            let expected = diagnostics.as_slice().to_vec();
            assert_diagnostic_sites(
                diagnostics.as_slice(),
                &[(Code::CheckType, 4, 12), (Code::CheckType, 8, 12)],
            );

            let as_ref: &[SourceDiagnostic] = diagnostics.as_ref();
            assert_eq!(as_ref, expected.as_slice());
            assert_eq!(diagnostics.iter().cloned().collect::<Vec<_>>(), expected);
            assert_eq!(
                (&diagnostics).into_iter().cloned().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                diagnostics.clone().into_iter().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(diagnostics.into_vec(), expected);
        }
        CompileFailure::ResourceLimit(_) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
        CompileFailure::Invariant(_) => {
            panic!("ordinary invalid source must remain a source-diagnostic failure")
        }
    }
}

const SHALLOW_SEED_TYPE_COUNT: usize = 64;

/// Build 4,096 distinct depth-one generic function instances without generic
/// recursion. The calls are divided among small functions so the fixture reaches
/// the shared instantiation bound without first crossing the independent
/// per-function code-byte bound. A caller appends the expression that requests the
/// next shared instantiation, so count-limit tests can isolate that exact source site.
fn shallow_function_reservation_fixture() -> String {
    let mut source = String::from(
        r#"module main

enum Held<T> {
    value(item: T)
}

"#,
    );
    for seed in 0..SHALLOW_SEED_TYPE_COUNT {
        writeln!(source, "struct N{seed} {{").expect("write generated seed declaration");
        source.push_str("    value: int\n}\n\n");
    }
    source.push_str(
        r#"fn prime<A, B>(a: A, b: B): int {
    return 0
}

"#,
    );
    // The 64 x 64 ordered pairs mint 4,096 distinct `prime<A, B>` rows.
    for left in 0..SHALLOW_SEED_TYPE_COUNT {
        writeln!(source, "fn reserve{left}(): int {{").expect("write reservation function");
        source.push_str("    var sink: int = 0\n");
        for right in 0..SHALLOW_SEED_TYPE_COUNT {
            writeln!(
                source,
                "    sink = prime(N{left}(value: 0), N{right}(value: 0))"
            )
            .expect("write generated prime application");
        }
        source.push_str("    return sink\n}\n\n");
    }
    source.push_str(
        r#"pub fn driver(): int {
    var sink: int = 0
"#,
    );
    source
}

/// A public generic function and every one of its monomorphized instances mint no
/// export (stable hash identity): only public monomorphic functions appear in the
/// export directory, however many times the generic is instantiated.
#[test]
fn generic_instances_mint_no_stable_identity() {
    let compiled = compile_ok(
        r#"module main

pub fn identity<T>(x: T): T {
    return x
}

pub fn driver(): int {
    const a = identity(1)
    const b = identity("two")
    const c = identity(true)
    const d = identity(a)
    return a
}
"#,
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
fn a_type_parameter_no_argument_determines_cannot_be_inferred() {
    let diagnostics = compile_err(
        r#"module main

fn make<T>(): T? {
    return absent
}

pub fn driver(): int {
    const x = make()
    return 0
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("cannot infer type parameter `T`")),
        "{diagnostics:#?}"
    );
}

/// The once-checked template pass rejects `==` over an unconstrained type
/// parameter, independently of whether the generic is ever instantiated.
#[test]
fn equality_on_an_unconstrained_parameter_is_rejected_in_the_body() {
    let diagnostics = compile_err(
        r#"module main

fn same<T>(a: T, b: T): bool {
    return a == b
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("supports equality")),
        "{diagnostics:#?}"
    );
}

/// The once-checked template pass rejects `<` over a parameter constrained only by
/// equality: order is a distinct constraint.
#[test]
fn order_on_an_equality_only_parameter_is_rejected_in_the_body() {
    let diagnostics = compile_err(
        r#"module main

fn smaller<T supports equality>(a: T, b: T): bool {
    return a < b
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("supports order")),
        "{diagnostics:#?}"
    );
}

/// A constrained generic body checks, but a call that instantiates the parameter
/// with a concrete type that does not support the constraint is revalidated and
/// rejected at the call site.
#[test]
fn a_call_revalidates_the_constraint_against_the_concrete_type() {
    // `bool` supports equality but not order: instantiating an order-constrained
    // parameter with `bool` is rejected per application.
    let diagnostics = compile_err(
        r#"module main

fn smaller<T supports order>(a: T, b: T): bool {
    return a < b
}

pub fn driver(): bool {
    return smaller(true, false)
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("does not `supports order`")),
        "{diagnostics:#?}"
    );
}

/// A generic body may call a monomorphic function that takes a concrete
/// collection type: the once-checked template pass sees the concrete callee's
/// collection type at the same index the real image records it, so the call is not
/// falsely rejected.
#[test]
fn a_generic_body_calls_a_concrete_collection_typed_function() {
    compile_ok(
        r#"module main

fn total(xs: List<int>): int {
    var sum: int = 0
    for x in xs {
        sum = sum + x
    }
    return sum
}

fn wrap<T>(x: T): int {
    var ns: List<int> = List()
    ns = append(ns, 1)
    ns = append(ns, 2)
    return total(ns)
}

pub fn driver(): int {
    return wrap(true)
}
"#,
    );
}

/// The same generic instantiated at two different concrete types both check: the
/// body is checked once against the constraint, and each application revalidates.
#[test]
fn a_constrained_generic_instantiates_at_several_supporting_types() {
    compile_ok(
        r#"module main

fn smaller<T supports order>(a: T, b: T): bool {
    return a < b
}

pub fn driver(): bool {
    const byInt = smaller(1, 2)
    const byText = smaller("a", "b")
    return byInt
}
"#,
    );
}

/// A generic self-call at the same instantiation is a recursion cycle over the
/// instance's image function, rejected as `check.recursion`.
#[test]
fn a_generic_recursing_at_the_same_instantiation_is_recursion() {
    let diagnostics = compile_err(
        r#"module main

fn spin<T>(x: T): T {
    return spin(x)
}

pub fn driver(): int {
    return spin(1)
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckRecursion),
        "{diagnostics:#?}"
    );
}

/// A generic that recurses over an ever-growing type diverges monomorphization;
/// the instantiation bound fails it with a typed `check.instantiation_limit`
/// rather than looping unboundedly.
#[test]
fn divergent_monomorphization_hits_the_instantiation_bound() {
    let diagnostics = compile_err(
        r#"module main

fn grow<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    return grow(xs)
}

pub fn driver(): int {
    return grow(1)
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckInstantiationLimit),
        "{diagnostics:#?}"
    );
}

/// A shared-count refusal after a nested `Option` mint and with an independently
/// queued follower rejects the current body and stops the FIFO before a later
/// instance can occupy an earlier reserved function index. Debug retains the
/// reserved-versus-emitted assertion, while release must have the same typed result.
#[test]
fn reserved_option_limit_rejects_the_body_without_unwinding() {
    let diagnostics = compile_err(
        r#"module main

fn identity<T>(x: T): T {
    return x
}

fn grow<T>(x: T): int {
    const y = some(x)
    const next = grow(y)
    const held = identity(x)
    const z = some(y)
    return next
}

pub fn driver(): int {
    return grow(1)
}
"#,
    );
    assert_one_located_limit(&diagnostics, 11, 20);
}

/// An Option-free fan-out leaves an already queued safe follower behind the first
/// body whose second reservation reaches the shared bound. The driver must stop at
/// that rejected reserved body before the follower can occupy its missing slot.
#[test]
fn function_limit_stops_before_an_option_free_queued_follower_fills_the_hole() {
    let diagnostics = compile_err(
        r#"module main

fn leaf<T>(x: T): int {
    return 0
}

fn grow<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    const next = grow(xs)
    const follower = leaf(x)
    return next
}

pub fn driver(): int {
    return grow(1)
}
"#,
    );
    assert_one_located_limit(&diagnostics, 11, 22);
    assert_eq!(
        diagnostics[0].message(),
        "generic instantiation reached the limit of 4096 distinct function and type instances"
    );
}

/// Two independent growing roots are both queued before either recursive body is
/// drained. Their shared instantiation budget has one owner: the first failed
/// recursive reservation reports one located limit, and the other body cannot add
/// a duplicate or continue into a secondary failure.
#[test]
fn two_queued_growing_bodies_share_one_instantiation_limit() {
    let diagnostics = compile_err(
        r#"module main

fn left<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    return left(xs)
}

fn right<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    return right(xs)
}

pub fn driver(): int {
    const first = left(1)
    const second = right(1)
    return first + second
}
"#,
    );
    assert_one_located_limit(&diagnostics, 6, 12);
}

/// A type-limit refusal can be left only in the registry when a bare-`?` constructor
/// result is unused. The lowering exit must still transfer it before `finish`; the
/// driver must then stop before either already-queued body is lowered into the slot.
#[test]
fn residual_type_limit_rejects_the_body_before_finish_and_queue_drain() {
    let diagnostics = compile_err(
        r#"module main

struct Seed<T> {
    value: T
}

struct Held<T> {
    value: T
}

fn leaf<T>(x: T): int {
    return 0
}

fn grow<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    const next = grow(xs)
    const follower = leaf(x)
    const dropped = Held(value: x)
    return next
}

pub fn driver(): int {
    const seed = Seed(value: 0)
    return grow(1)
}
"#,
    );
    assert_one_located_limit(&diagnostics, 20, 21);
}

/// The recursive generic every depth-limit fixture names. Resolving `Grow<int>` reaches
/// the shared instantiation depth bound, whatever construct asks for it.
const GROW: &str = "struct Grow<T> {\n    next: Grow<List<T>>\n}\n";

/// One site that names [`GROW`], and where the single located limit it must report sits.
struct DepthLimitSite {
    /// The construct under test, named in a failure.
    site: &'static str,
    /// The source written after `GROW`; its first line is source line 7.
    tail: &'static str,
    /// Line and column of the one `check.instantiation_limit` row.
    at: (u32, u32),
}

/// Every resolution path that can reach the depth bound while building a signature, an
/// annotation, or a declared field. A refusal there is the limit itself: it may not be
/// substituted with Unit, may not drop the parameter or return it annotates, may not be
/// reclassified as the declaring pass's contextual `check.unsupported`, and may not be
/// followed by a second diagnostic derived from the construct it rejected.
const RESOLUTION_SITES: &[DepthLimitSite] = &[
    DepthLimitSite {
        site: "a generic function's return annotation",
        tail: r#"fn deepen<T>(x: T): Grow<T> {
    return deepen(x)
}

pub fn driver(): int {
    const ignored = deepen(1)
    return 0
}
"#,
        at: (7, 21),
    },
    DepthLimitSite {
        site: "a monomorphic signature parameter",
        tail: r#"fn take(value: Grow<int>): int {
    return 0
}

pub fn driver(): int {
    return take(0)
}
"#,
        at: (7, 16),
    },
    DepthLimitSite {
        site: "a monomorphic signature return",
        tail: r#"fn make(): Grow<int> {
    unreachable("unreachable fixture")
}

pub fn driver(): int {
    const value = make()
    return 0
}
"#,
        at: (7, 12),
    },
    DepthLimitSite {
        site: "a local binding's explicit annotation",
        tail: r#"pub fn driver(): int {
    const value: Grow<int> = 0
    return 0
}
"#,
        at: (8, 18),
    },
    DepthLimitSite {
        site: "a checked-result `const` annotation",
        tail: r#"pub fn driver(): int {
    const value: Grow<int> = checked 1 + 2
        on out_of_range return 0
    return value.next
}
"#,
        at: (8, 18),
    },
    DepthLimitSite {
        site: "a checked-result `var` annotation",
        tail: r#"pub fn driver(): int {
    var value: Grow<int> = checked 1 + 2
        on out_of_range return 0
    return value.next
}
"#,
        at: (8, 16),
    },
    DepthLimitSite {
        site: "an `if const` annotation",
        tail: r#"pub fn driver(): int {
    const maybe: int? = 1
    if const value: Grow<int> = maybe {
        return value
    } else {
        return 0
    }
}
"#,
        at: (9, 21),
    },
    DepthLimitSite {
        site: "a concrete struct field",
        tail: r#"struct Holder {
    value: Grow<int>
}

pub fn driver(): int {
    return 0
}
"#,
        at: (8, 12),
    },
    DepthLimitSite {
        site: "a resource field",
        tail: r#"resource Holder {
    value: Grow<int>
}

pub fn driver(): int {
    return 0
}
"#,
        at: (8, 12),
    },
    DepthLimitSite {
        site: "a group leaf",
        tail: r#"resource Holder {
    details {
        value: Grow<int>
    }
}

pub fn driver(): int {
    return 0
}
"#,
        at: (9, 16),
    },
];

#[test]
fn every_resolution_site_reports_one_located_depth_limit() {
    for case in RESOLUTION_SITES {
        assert_one_site_limit(case);
    }
}

/// The one located `check.instantiation_limit` a depth-limit site owes, named by the site
/// so a failure says which one broke.
fn assert_one_site_limit(case: &DepthLimitSite) {
    let diagnostics = compile_err(&format!("module main\n\n{GROW}\n{}", case.tail));
    let located: Vec<(Code, &str, u32, u32)> = diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code(),
                diagnostic.file().as_str(),
                diagnostic.line(),
                diagnostic.column(),
            )
        })
        .collect();
    assert_eq!(
        located,
        vec![(
            Code::CheckInstantiationLimit,
            "src/main.mw",
            case.at.0,
            case.at.1,
        )],
        "{}: exactly one located instantiation limit, with nothing derived from the \
         construct it rejected: {diagnostics:#?}",
        case.site,
    );
}

/// A generic return instantiated from another module must keep the return template's
/// real file together with its span. Pairing the caller's file with the template's
/// line and column would manufacture a location that exists in neither source site.
#[test]
fn cross_file_generic_return_limit_keeps_one_coherent_template_location() {
    let diagnostics = compile_files_err(&[
        (
            "src/library.mw",
            r#"module library

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn deepen<T>(x: T): Grow<T> {
    return deepen(x)
}
"#,
        ),
        (
            "src/main.mw",
            r#"module main
use library

pub fn driver(): int {
    const ignored = library::deepen(1)
    return 0
}
"#,
        ),
    ]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code(), Code::CheckInstantiationLimit);
    assert_eq!(diagnostic.file().as_str(), "src/library.mw");
    assert_eq!((diagnostic.line(), diagnostic.column()), (7, 25));
}

/// A direct generic-struct construction that reaches the shared count bound must
/// reject its body at the mint. The missing construction cannot fall through and
/// turn the later field use into a secondary name/type diagnostic.
#[test]
fn count_limit_at_a_direct_generic_struct_construction_rejects_the_body() {
    let diagnostics = compile_err(
        r#"module main

struct Held<T> {
    value: T
}

fn identity<T>(x: T): T {
    return x
}

fn grow<T>(x: T): int {
    var xs: List<T> = List()
    xs = append(xs, x)
    const held = Held(value: x)
    const observed = identity(held)
    return grow(xs)
}

pub fn driver(): int {
    return grow(1)
}
"#,
    );
    assert_one_located_limit(&diagnostics, 14, 18);
    assert_eq!(
        diagnostics[0].message(),
        "generic instantiation reached the limit of 4096 distinct function and type instances"
    );
}

/// The sibling direct generic-enum constructor has the same refusal transfer: a
/// failed mint cannot leave the binding absent and let the following match invent
/// a secondary diagnostic.
#[test]
fn count_limit_at_a_direct_generic_enum_construction_rejects_the_body() {
    let mut source = shallow_function_reservation_fixture();

    let constructor_line = source.lines().count() as u32 + 1;
    assert_eq!(constructor_line, 4685, "generated source layout drifted");
    source.push_str(
        r#"    const held = Held::value(item: sink)
    match held {
        value(item) => {
            return item
        }
    }
}
"#,
    );
    assert!(
        source.len() < CaptureLimits::DEFAULT.max_file_bytes(),
        "generated source must stay within the captured-file bound"
    );

    let diagnostics = compile_err(&source);
    assert_one_located_limit(&diagnostics, constructor_line, 18);
    assert_eq!(
        diagnostics[0].message(),
        "generic instantiation reached the limit of 4096 distinct function and type instances"
    );
}

/// Interpolation ordinarily accumulates independent part diagnostics. A shared
/// instantiation-limit refusal is terminal instead: once the first hole refuses,
/// no later hole may manufacture a secondary diagnostic from a rejected body.
#[test]
fn interpolation_stops_after_a_part_reaches_the_instantiation_limit() {
    let mut source = shallow_function_reservation_fixture();
    let constructor_line = source.lines().count() as u32 + 1;
    assert_eq!(constructor_line, 4685, "generated source layout drifted");
    source.push_str(
        r#"    const rendered = $"{Held::value(item: sink)} {missing()}"
    return 0
}
"#,
    );
    assert!(
        source.len() < CaptureLimits::DEFAULT.max_file_bytes(),
        "generated source must stay within the captured-file bound"
    );

    let diagnostics = compile_err(&source);
    assert_one_located_limit(&diagnostics, constructor_line, 25);
}

/// Ordinary failed holes remain independent: only the terminal shared limit changes
/// interpolation's established multi-part diagnostic accumulation.
#[test]
fn interpolation_keeps_accumulating_ordinary_part_diagnostics() {
    let diagnostics = compile_err(
        r#"module main

pub fn driver(): string {
    return $"{firstMissing()} {secondMissing()}"
}
"#,
    );
    assert_diagnostic_sites(
        &diagnostics,
        &[(Code::CheckType, 4, 15), (Code::CheckType, 4, 32)],
    );
}

/// A recursive type fill that refuses at a function signature must reject that
/// signature before the placeholder-looking body can be lowered. This pins one
/// located limit with no false field cascade; replay and cache mechanics belong to
/// private owner KATs.
#[test]
fn a_failed_recursive_type_fill_rejects_the_signature_before_body_lowering() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn consume(value: Grow<int>): int {
    value.next
    return 0
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 7, 19);
}

/// Collection-payload `Unsupported` is independent of a later instantiation
/// limit. It must not occupy the limit owner's single pending slot and suppress the
/// located limit; cross-family order is canonical limit before payload.
#[test]
fn payload_rejection_before_a_limit_preserves_both_diagnostic_families() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn payloadOne(value: Option<List<int>>): int {
    return 0
}

fn payloadTwo(value: Result<int, Map<int, int>>): int {
    return 0
}

fn depth(value: Grow<int>): int {
    return 0
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_diagnostic_sites(
        &diagnostics,
        &[
            (Code::CheckInstantiationLimit, 15, 17),
            (Code::CheckUnsupported, 7, 22),
            (Code::CheckUnsupported, 11, 22),
        ],
    );
}

/// The reverse source order has the same canonical cross-family order and does not
/// deduplicate the independent collection-payload rejection into the limit.
#[test]
fn limit_before_a_payload_rejection_preserves_both_diagnostic_families() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn depth(value: Grow<int>): int {
    return 0
}

fn payloadOne(value: Option<List<int>>): int {
    return 0
}

fn payloadTwo(value: Result<int, Map<int, int>>): int {
    return 0
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_diagnostic_sites(
        &diagnostics,
        &[
            (Code::CheckInstantiationLimit, 7, 17),
            (Code::CheckUnsupported, 11, 22),
            (Code::CheckUnsupported, 15, 22),
        ],
    );
}

/// Ordinary unsupported generic applications remain contextual `check.unsupported`
/// and do not get promoted into the instantiation-limit family.
#[test]
fn ordinary_unsupported_generic_application_stays_unsupported() {
    let diagnostics = compile_err(
        r#"module main

struct Pair<T> {
    value: T
}

pub fn driver(value: Pair<int, string>): int {
    return 0
}
"#,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code(), Code::CheckUnsupported);
    assert_eq!(diagnostics[0].file().as_str(), "src/main.mw");
    assert_eq!((diagnostics[0].line(), diagnostics[0].column()), (7, 22));
}

/// A genuinely unsupported checked-result annotation remains the contextual
/// `check.unsupported` owned by `coerce_int_result`; the limit-only transfer rules
/// must not suppress or relabel it.
#[test]
fn checked_annotation_keeps_a_genuine_unsupported_contextual() {
    let diagnostics = compile_err(
        r#"module main

struct Pair<T> {
    value: T
}

pub fn driver(): int {
    const value: Pair<int, string> = checked 1 + 2
        on out_of_range return 0
    return value
}
"#,
    );
    assert_diagnostic_sites(&diagnostics, &[(Code::CheckUnsupported, 8, 18)]);
}

/// An unused generic template is checked inside an isolated savepoint over the live
/// registry and draft. Its proof-local type limit and collection-payload refusal must
/// both transfer to the real diagnostic coordinator in canonical limit-before-payload
/// order. The later safe export only keeps the fixture free of an unrelated body
/// diagnostic.
#[test]
fn template_proof_transfers_its_limit_before_its_payload_diagnostic() {
    let diagnostics = compile_err(
        r#"module main

fn payload<T>(value: Option<List<T>>): int {
    return 0
}

struct Grow<T> {
    next: Grow<List<T>>
}

fn deepen<T>(x: T): Grow<T> {
    return deepen(x)
}

pub fn safe(): int {
    return 0
}
"#,
    );
    assert_diagnostic_sites(
        &diagnostics,
        &[
            (Code::CheckInstantiationLimit, 11, 21),
            (Code::CheckUnsupported, 3, 22),
        ],
    );
}

/// A body-time limit in an ordinary monomorphic function is taken before visiting
/// later ordinary or test bodies. The concrete `Cycle` makes the currently
/// unconditional value-cycle audit observable; that audit is also downstream of the
/// stopped body pass.
#[test]
fn ordinary_body_limit_stops_later_bodies_and_the_value_cycle_audit() {
    let diagnostics = compile_tests_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn first(): int {
    const value: Grow<int> = 0
    return 0
}

pub fn laterExport(): int {
    const queued = identity(1)
    return missing()
}

test "later test" {
    const queued = identity(2)
    assert missing()
}

fn identity<T>(x: T): T {
    return x
}

struct Cycle {
    next: Cycle
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 18);
}

/// Test bodies use the same coordinator boundary. Once the first test reaches a
/// type limit, no later test body is visited, and the currently unconditional
/// value-cycle audit remains downstream of that stop.
#[test]
fn test_body_limit_stops_later_test_bodies_and_the_value_cycle_audit() {
    let diagnostics = compile_tests_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

test "first" {
    const value: Grow<int> = 0
}

test "later" {
    const queued = identity(1)
    assert missing()
}

fn identity<T>(x: T): T {
    return x
}

struct Cycle {
    next: Cycle
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 18);
}

/// Every nested control owner that can hold a depth-limit refusal. Each body places a
/// `missing()` call after the refusing construct: if the owner kept visiting after the
/// limit, that call — or the owner's own exhaustiveness, fallthrough, or divergence
/// check — would add a second row, so the single-row assertion is what pins the stop.
const NESTED_STOP_SITES: &[DepthLimitSite] = &[
    DepthLimitSite {
        site: "a generic template's parameter, before its first body statement",
        tail: r#"fn inspect<T>(value: Grow<T>): int {
    return missing()
}

pub fn safe(): int {
    return 0
}
"#,
        at: (7, 22),
    },
    DepthLimitSite {
        site: "an `if` branch",
        tail: r#"pub fn driver(flag: bool): int {
    if flag {
        const value: Grow<int> = 0
        return 1
    } else {
        return missing()
    }
}
"#,
        at: (9, 22),
    },
    DepthLimitSite {
        site: "the present branch of `if const`",
        tail: r#"pub fn driver(): int {
    const maybe: int? = 1
    if const present = maybe {
        const value: Grow<int> = 0
        return 1
    } else {
        return missing()
    }
}
"#,
        at: (10, 22),
    },
    DepthLimitSite {
        site: "a match arm",
        tail: r#"enum Choice {
    first
    second
}

pub fn driver(choice: Choice): int {
    match choice {
        first => {
            const value: Grow<int> = 0
            return 1
        }
        second => return missing()
    }
}
"#,
        at: (15, 26),
    },
    DepthLimitSite {
        site: "a checked-fault handler branch",
        tail: r#"pub fn driver(flag: bool): int {
    const value = checked 1 + 2
        on out_of_range {
            if flag {
                const nested: Grow<int> = 0
            } else {
                return missing()
            }
        }
    return value
}
"#,
        at: (11, 31),
    },
];

#[test]
fn every_nested_control_owner_stops_at_a_depth_limit() {
    for case in NESTED_STOP_SITES {
        assert_one_site_limit(case);
    }
}

/// Reusing one rejected generic application at two real signature consumers keeps
/// cache identity private while each consumer owns a truthful contextual
/// `check.unsupported` at its current source site.
#[test]
fn rejected_unsupported_replay_is_contextual_at_each_consumer_site() {
    let diagnostics = compile_err(
        r#"module main

struct Bad<T> {
    broken: Missing<T>
}

fn first(value: Bad<int>): int {
    return 0
}

fn second(value: Bad<int>): int {
    return 0
}

pub fn safe(): int {
    return 0
}
"#,
    );
    // A template member naming a type nothing declares is reported at the
    // declaration, once, and the uses are steered to it. The second use is silent:
    // the steer is once per refused key.
    assert_diagnostic_sites(
        &diagnostics,
        &[(Code::CheckType, 4, 13), (Code::CheckType, 7, 17)],
    );
}

/// `Inner<int>` can finish locally while its `outer` field points at the in-progress
/// `Outer<int>`. If `Outer<int>` later fails through its `bad` sibling, production
/// readers must not expose that failed placeholder through a Ready-looking
/// `Inner<int>`. Atomic rejection of every provisional row in the mutually recursive
/// dependency closure is left to the required private type-owner KAT
/// `failed_fill_rejects_reverse_dependent_rows_without_poisoning_siblings`:
/// the failed `Outer` and completed dependent `Inner` are rejected, an independent
/// `Good` remains ready, no unresolved filling state remains, and the fill stack is
/// empty. Those private state assertions are not claims of this boundary test.
#[test]
fn completed_inner_reuse_never_exposes_failed_outer_placeholder() {
    let diagnostics = compile_err(
        r#"module main

struct Diverge<T> {
    next: Diverge<List<T>>
}

struct Outer<T> {
    inner: Inner<T>
    bad: Diverge<T>
}

struct Inner<T> {
    outer: Outer<T>
}

fn consume(value: Outer<int>): int {
    return 0
}

fn reuse(value: Inner<int>): int {
    value.outer.inner
    return 0
}

pub fn safe(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 16, 19);
}

/// `Bad<int>` names an unavailable nested generic type. Resolving that application
/// at a function parameter must produce the contextual `Unsupported` at that
/// observable current site instead of exposing an incomplete type. Row identity,
/// remint behavior, cache length and depth, and replay mechanics belong to private
/// owner KATs rather than this production-path check.
#[test]
fn a_parameter_bound_nested_unsupported_resolution_is_contextually_refused() {
    let diagnostics = compile_err(
        r#"module main

struct Good<T> {
    value: T
}

struct Bad<T> {
    good: Good<T>
    broken: Missing<T>
}

fn useBad(value: Bad<int>): int {
    return 0
}

fn useGood(value: Good<int>): int {
    return value.value
}
"#,
    );
    // `Missing<T>` is refused where it is written, and `useBad`'s annotation reuses
    // that cause rather than reporting a subset gap for a template this project
    // declared.
    assert_diagnostic_sites(
        &diagnostics,
        &[(Code::CheckType, 9, 13), (Code::CheckType, 12, 18)],
    );
}

// --- user-definable generic value types (slice 3) ---

/// A generic `struct` and `enum` are templates, not concrete image types: they mint
/// nothing until used, and neither a template nor any of its instantiations is a
/// stable export. The only exports are the monomorphic `pub` functions.
#[test]
fn generic_type_instantiations_mint_no_stable_identity() {
    let compiled = compile_ok(
        r#"module main

struct Pair<A, B> {
    first: A
    second: B
}

enum Box<T> {
    empty
    full(value: T)
}

pub fn run(): int {
    const p = Pair(first: 1, second: "x")
    const q = Pair(first: true, second: 2)
    const b = Box::full(value: 9)
    return p.first
}
"#,
    );
    let exports: Vec<&str> = compiled.exports.iter().map(|e| e.item.as_str()).collect();
    assert_eq!(exports, vec!["run"]);
}

/// A generic struct field is read at the concrete substituted type; a wrong field
/// name is a typed error against the instantiation, not a panic.
#[test]
fn a_generic_struct_field_is_typed_by_its_instantiation() {
    let diagnostics = compile_err(
        r#"module main

struct Wrapper<T> {
    value: T
}

pub fn run(): int {
    const w = Wrapper(value: 3)
    return w.missing
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
}

/// A generic type's `supports order` constraint is revalidated at construction: an
/// argument that does not support ordering is rejected.
#[test]
fn a_generic_type_constraint_is_revalidated_at_construction() {
    let diagnostics = compile_err(
        r#"module main

struct Ordered<T supports order> {
    lo: T
    hi: T
}

struct Point {
    x: int
}

pub fn run(): int {
    const o = Ordered(lo: Point(x: 1), hi: Point(x: 2))
    return 0
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
}

/// A monomorphized generic type cycle (`Tree[int]` directly containing `Tree[int]`)
/// is an ordinary value cycle per instantiation and is rejected as recursion at the
/// template's declaration.
#[test]
fn a_generic_type_containing_itself_is_a_value_cycle() {
    let diagnostics = compile_err(
        r#"module main

struct Tree<T> {
    value: T
    child: Tree<T>
}

fn useTree(t: Tree<int>): int {
    return t.value
}

pub fn run(): int {
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckRecursion),
        "{diagnostics:#?}"
    );
}

/// A nested generic value cycle renders every instantiation on the reported
/// `check.recursion` path in the canonical angle form: the cycle through
/// `Loop<int>` and the nested `Box<Loop<int>>` names both with angle delimiters at
/// every level, the same display form the checker uses for all other generic labels.
#[test]
fn a_nested_generic_value_cycle_labels_instantiations_in_angle_form() {
    let diagnostics = compile_err(
        r#"module main

struct Loop<T> {
    step: Box<Loop<T>>
}

struct Box<T> {
    held: T
}

fn useLoop(l: Loop<int>): int {
    return 0
}

pub fn run(): int {
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckRecursion),
        "{diagnostics:#?}"
    );
    let cycle = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code() == Code::CheckRecursion)
        .expect("a recursion diagnostic");
    assert!(
        cycle.message().contains("Loop<int>"),
        "the cycle path must render `Loop<int>` in angle form: {}",
        cycle.message()
    );
    assert!(
        cycle.message().contains("Box<Loop<int>>"),
        "the cycle path must render the nested `Box<Loop<int>>` in angle form: {}",
        cycle.message()
    );
}

/// A nested collection instantiation is named in checker diagnostics in the
/// canonical angle form at every level: a `Map<string, List<int>>` value bound to an
/// `int` renders `Map<string, List<int>>`, including the nested `List<int>` value
/// type, rather than a bracket spelling.
#[test]
fn a_nested_collection_type_is_named_in_angle_form() {
    let diagnostics = compile_err(
        r#"module main

pub fn run(): int {
    const m: Map<string, List<int>> = Map()
    return m
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message().contains("Map<string, List<int>>")),
        "the collection type must render in angle form: {diagnostics:#?}"
    );
}

/// A nested `try` error mismatch names its `Result` operands in the canonical angle
/// form: when the propagated error type is itself a `Result<int, string>`, the typed
/// `check.type` message renders `Result<int, string>` at every level, not a bracket
/// spelling. This is the compiler diagnostic owner — `marrow run` projects
/// diagnostics to KAT-frozen code+span records, so the operand is asserted here at
/// `compile`, not through the binary.
#[test]
fn a_nested_try_error_mismatch_names_result_operands_in_angle_form() {
    let diagnostics = compile_err(
        r#"module main

fn g(n: int): Result<int, Result<int, string>> {
    return ok(n)
}

pub fn f(): Result<int, int> {
    const x = try g(1)
    return ok(x)
}
"#,
    );
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message().contains("Result<int, string>")),
        "the propagated error operand must render in angle form: {diagnostics:#?}"
    );
}

/// A cycle broken by a collection (`struct Node[T]` whose field is `List[Node[T]]`)
/// is a finite value and is admitted: a list terminates, so it adds no containment
/// edge.
#[test]
fn a_generic_type_cycle_through_a_collection_is_admitted() {
    compile_ok(
        r#"module main

struct Node<T> {
    value: T
    kids: List<Node<T>>
}

pub fn run(): int {
    var kids: List<Node<int>> = List()
    const n = Node(value: 1, kids: kids)
    return n.value
}
"#,
    );
}

/// A generic type recursing over an ever-growing argument (`Grow[T]` whose field is
/// `Grow[List[T]]`) diverges under monomorphization and hits the shared
/// instantiation bound rather than looping.
#[test]
fn a_divergent_generic_type_hits_the_instantiation_bound() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    value: T
    next: Grow<List<T>>
}

fn useGrow(g: Grow<int>): int {
    return g.value
}

pub fn run(): int {
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckInstantiationLimit),
        "{diagnostics:#?}"
    );
}

/// `Option` and `Result` are ordinary generic enums the toolchain registers, not a
/// built-in special case: a user cannot redeclare their reserved names.
#[test]
fn the_reserved_generic_names_cannot_be_redeclared() {
    let diagnostics = compile_err(
        r#"module main

enum Option<T> {
    nothing
    something(value: T)
}

pub fn run(): int {
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, Code::CheckNameConflict),
        "{diagnostics:#?}"
    );
}

/// `Option[Option[int]]` is a distinct instantiation from `Option[int]`: the reserved
/// generic enum monomorphizes by argument exactly like a user generic enum.
#[test]
fn nested_option_is_a_distinct_instantiation() {
    compile_ok(
        r#"module main

pub fn run(): int {
    const inner: Option<int> = some(1)
    const outer: Option<Option<int>> = some(inner)
    match outer {
        none => return 0
        some(v) => {
            match v {
                none => return 0
                some(k) => return k
            }
        }
    }
}
"#,
    );
}

/// One position where a collection would become an enum-payload leaf, and the naming the
/// refusal must carry.
struct PayloadRefusal {
    /// The payload position under test.
    position: &'static str,
    source: &'static str,
    /// Fragments one rendered row must carry together, so the reader is told which
    /// member holds the leaf the image cannot represent.
    naming: &'static [&'static str],
}

/// The image admits a scalar, record, or enum enum-payload leaf; a collection is not one.
/// Every position that can carry one is refused at check time, so a checker-clean program
/// can never mint an image the verifier rejects at the Table phase.
const COLLECTION_PAYLOAD_REFUSALS: &[PayloadRefusal] = &[
    PayloadRefusal {
        position: "a bare `some(...)` constructor",
        source: r#"module main

pub fn run(): int {
    const x = some(List(1, 2, 3))
    return 0
}
"#,
        naming: &["`some` payload of `Option`", "not a payload type"],
    },
    PayloadRefusal {
        position: "an `Option<List<int>>` annotation carrying no constructor",
        source: r#"module main

pub fn run(): int {
    const x: Option<List<int>> = none
    return 0
}
"#,
        naming: &["not a payload type"],
    },
    PayloadRefusal {
        position: "a `Result` whose `ok` payload monomorphizes to a `Map`",
        source: r#"module main

pub fn run(): Result<Map<int, int>, int> {
    return ok(Map())
}
"#,
        naming: &["`ok` payload of `Result`", "`Map`"],
    },
    PayloadRefusal {
        position: "a user generic enum instantiated at a collection argument",
        source: r#"module main

enum Box<T> {
    wrap(v: T)
}

pub fn run(): int {
    const x = Box::wrap(v: List(1, 2, 3))
    return 0
}
"#,
        naming: &["`wrap` payload of `Box`"],
    },
    PayloadRefusal {
        position: "a user generic enum whose template body wraps its parameter in a collection",
        source: r#"module main

enum E<T> {
    v(x: List<T>)
}

pub fn run(): int {
    const x = E::v(x: List(1))
    return 0
}
"#,
        naming: &["`v` payload of `E`"],
    },
    PayloadRefusal {
        position: "a collection buried under nested `Option` layers",
        source: r#"module main

pub fn run(): int {
    const x = some(some(List(1, 2, 3)))
    return 0
}
"#,
        naming: &["not a payload type"],
    },
    PayloadRefusal {
        position: "a function parameter typed `Option<List<int>>`",
        source: r#"module main

pub fn takes(o: Option<List<int>>): int {
    return 0
}

pub fn run(): int {
    return 0
}
"#,
        naming: &["not a payload type"],
    },
];

#[test]
fn every_collection_payload_position_is_refused_by_name() {
    for case in COLLECTION_PAYLOAD_REFUSALS {
        let diagnostics = compile_err(case.source);
        assert!(
            has_code(&diagnostics, Code::CheckUnsupported),
            "{}: the collection payload leaf is refused: {diagnostics:#?}",
            case.position,
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                case.naming
                    .iter()
                    .all(|fragment| diagnostic.message().contains(fragment))
            }),
            "{}: one row names the refused payload {:?}: {diagnostics:#?}",
            case.position,
            case.naming,
        );
    }
}

/// An `Option` of a struct keeps compiling: a struct is an admitted enum-payload
/// leaf, so wrapping a collection in a struct is the stated fix and it works.
#[test]
fn an_option_of_a_struct_holding_a_collection_compiles() {
    compile_ok(
        r#"module main

struct Items {
    xs: List<int>
}

pub fn run(): int {
    const x = some(Items(xs: List(1, 2, 3)))
    return 0
}
"#,
    );
}

/// A `List` field on a struct is unaffected: the enum-payload restriction does not
/// touch struct fields, which admit collections.
#[test]
fn a_struct_field_collection_still_compiles() {
    compile_ok(
        r#"module main

struct Items {
    xs: List<int>
}

pub fn run(): int {
    const x = Items(xs: List(1, 2, 3))
    return 0
}
"#,
    );
}

/// A generic-heavy program that settles many instantiations at declare time and then runs
/// several once-checked template proofs over that population. The proofs run directly on
/// the in-progress registry and draft inside a savepoint, not on a per-template clone, so
/// this is the accepted-program byte-identity fence for that path: the encoded image bytes
/// are frozen, and any perturbation of the proof pass that leaked into the real image fails
/// here. The digest is a hash of the full encoded image; a single changed byte changes it.
#[test]
fn a_generic_heavy_program_has_frozen_image_bytes() {
    let compiled = compile_ok(
        r#"module main

struct Held<T> { value: T }
struct Pair<T> {
    a: T
    b: T
}

fn identity<T>(x: T): T { return x }
fn firstOf<T>(p: Pair<T>): T { return p.a }
fn boxed<T>(x: T): Held<T> { return Held(value: x) }

struct Records {
    i: Held<int>
    s: Held<string>
    b: Held<bool>
    pi: Pair<int>
    ps: Pair<string>
}

pub fn driver(): int {
    const a = identity(1)
    const b = identity(true)
    const c = firstOf(Pair(a: 1, b: 2))
    const d = boxed("x")
    return a + c
}
"#,
    );
    let bytes = compiled.image.bytes;
    let hex: String = marrow_image::image_id(&bytes)
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        hex,
        "1322c7c6621ced42ef4c88dd31c73e235a2f09597af3b6c2c6c0128bba9a9a5d",
        "generic-heavy image bytes changed; the template-proof savepoint must not perturb the \
         accepted image (encoded {} bytes)",
        bytes.len(),
    );
}

/// A generic-heavy program whose one failing template proof (`<` on an equality-only
/// parameter) sits beside good templates and a settled instantiation population. The failed
/// proof rolls back through the savepoint, so it corrupts neither the sibling templates'
/// proofs nor the concrete work: the diagnostic set is exactly the one located failure. This
/// is the diagnostic-identity fence for the failing-proof path.
#[test]
fn a_failing_template_proof_is_contained_beside_generic_heavy_work() {
    let diagnostics = compile_err(
        r#"module main

struct Held<T> { value: T }

fn identity<T>(x: T): T { return x }
fn boxed<T>(x: T): Held<T> { return Held(value: x) }
fn bad<T supports equality>(a: T, b: T): bool { return a < b }

struct Records {
    i: Held<int>
    s: Held<string>
    b: Held<bool>
}

pub fn driver(): int {
    const a = identity(1)
    const d = boxed("x")
    return a
}
"#,
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "the failed proof must not cascade into sibling templates or concrete work: {diagnostics:#?}"
    );
    assert_eq!(diagnostics[0].code(), Code::CheckType);
    assert!(
        diagnostics[0].message().contains("supports order"),
        "{diagnostics:#?}"
    );
    assert_eq!(
        (diagnostics[0].line(), diagnostics[0].column()),
        (7, 60),
        "the located failure points at the `<` in the bad template body: {diagnostics:#?}"
    );
}

/// A type parameter's declaration position is a distinct abstract identity at any
/// admitted width: with 65,537 parameters in one admitted source file, the parameter
/// at position 65,536 is not the parameter at position 0, so returning the first
/// where the last is expected is the same `check.type` mismatch the two-parameter
/// control below proves — never a silent alias of ordinal 0.
#[test]
fn a_type_parameter_past_the_u16_domain_does_not_alias_ordinal_zero() {
    // The control: the shape at width two is a mismatch.
    let diagnostics =
        compile_err("module main\n\nfn wrap<A, B>(a: A, b: B): B {\n    return a\n}\n");
    assert!(has_code(&diagnostics, Code::CheckType), "{diagnostics:#?}");

    // The same shape at width 65,537: the last parameter's position exceeds `u16`.
    let mut source = String::from("module main\n\nfn wrap<");
    for index in 0..=65_536u32 {
        if index > 0 {
            source.push_str(", ");
        }
        write!(source, "T{index}").expect("write generated parameter");
    }
    source.push_str(">(a: T0, b: T65536): T65536 {\n    return a\n}\n");
    let diagnostics = compile_err(&source);
    assert!(
        has_code(&diagnostics, Code::CheckType),
        "the position-65,536 parameter must not alias ordinal 0: {diagnostics:#?}",
    );
}

#[test]
fn aliases_and_type_parameters_keep_distinct_bindings_in_type_templates() {
    for declaration in [
        "struct Box<T> { value: T }",
        "enum Box<T> { value(value: T) }",
    ] {
        let constructor = if declaration.starts_with("struct") {
            "Box(value: true)"
        } else {
            "Box::value(value: true)"
        };
        compile_ok(&format!(
            "module main\nalias T = int\n{declaration}\npub fn driver(): int {{ const value: Box<bool> = {constructor}\nreturn 0 }}\n"
        ));
    }
    for (declaration, constructor) in [
        (
            "struct Box<Item> { local: Item\nsaved: Saved }",
            "Box(local: true, saved: Item(value: 1))",
        ),
        (
            "enum Box<Item> { value(local: Item, saved: Saved) }",
            "Box::value(local: true, saved: Item(value: 1))",
        ),
    ] {
        let source = format!(
            "module main\nstruct Item {{ value: int }}\nalias Direct = Item\nalias Saved = Direct\n{declaration}\npub fn driver(): int {{ const value: Box<bool> = {constructor}\nreturn 0 }}\n"
        );
        compile_ok(&source);
        let diagnostics = compile_err(&source.replace("saved: Item(value: 1)", "saved: false"));
        assert!(
            diagnostics.iter().any(|row| row.code() == Code::CheckType),
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn optional_aliases_remain_global_in_generic_function_annotations() {
    compile_ok(
        "module main\nstruct Item { value: int }\nalias Saved = Item?\nfn keep<Item>(local: Item): Saved { const saved: Saved = absent\nreturn saved }\npub fn driver(): Item? { return keep(true) }\n",
    );
    let diagnostics = compile_err(
        "module main\nalias Saved = int?\nfn bad<T>(value: Saved?): int { return 0 }\npub fn driver(): int { return 0 }\n",
    );
    assert!(
        diagnostics
            .iter()
            .any(|row| row.code() == Code::CheckUnsupported)
    );
}

#[test]
fn forward_aliases_preserve_all_admitted_global_type_families() {
    compile_ok(
        "module main\nalias N = Number\nalias S = Product\nalias E = Choice\nalias R = Record\ntype Number: int in 0..=10\nstruct Product { value: int }\nenum Choice { item(value: int) }\nresource Record { required value: int }\nfn number(value: N): N { return value }\nfn product(value: S): S { return value }\nfn choice(value: E): E { return value }\nfn record(value: R): R { return value }\npub fn driver(): int { return 0 }\n",
    );
}
