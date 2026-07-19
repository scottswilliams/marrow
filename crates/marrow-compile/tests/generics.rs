//! Rank-1 generic function checking and monomorphization through the production
//! `compile` path: type-argument inference, the once-checked template pass against
//! `supports equality`/`supports order` constraints, per-application revalidation,
//! the instantiation bound, and the image-local (no stable identity) nature of
//! monomorphized instances.

use marrow_compile::compile_with_tests;
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

fn compile_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the program compiled"),
        Err(diagnostics) => {
            assert!(
                !diagnostics.is_empty(),
                "the public compiler failure boundary returned Err([])"
            );
            diagnostics
        }
    }
}

fn compile_tests_err(source: &str) -> Vec<SourceDiagnostic> {
    match compile_with_tests(&project(source)) {
        Ok(_) => panic!("expected a diagnostic, but the project tests compiled"),
        Err(diagnostics) => {
            assert!(
                !diagnostics.is_empty(),
                "the public compiler-with-tests failure boundary returned Err([])"
            );
            diagnostics
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
        Err(diagnostics) => {
            assert!(
                !diagnostics.is_empty(),
                "the public multi-file compiler failure boundary returned Err([])"
            );
            diagnostics
        }
    }
}

fn has_code(diagnostics: &[SourceDiagnostic], code: &str) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

fn assert_one_located_limit(diagnostics: &[SourceDiagnostic], line: u32, column: u32) {
    assert_eq!(
        diagnostics.len(),
        1,
        "a limit refusal must not cascade: {diagnostics:#?}"
    );
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code, "check.instantiation_limit");
    assert_eq!(diagnostic.file, "src/main.mw");
    assert_eq!((diagnostic.line, diagnostic.column), (line, column));
}

fn assert_diagnostic_sites(diagnostics: &[SourceDiagnostic], expected: &[(&str, u32, u32)]) {
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.file == "src/main.mw"),
        "every diagnostic must retain the source file: {diagnostics:#?}"
    );
    let actual: Vec<(&str, u32, u32)> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.line, diagnostic.column))
        .collect();
    assert_eq!(actual, expected, "{diagnostics:#?}");
}

const SHALLOW_SEED_TYPE_COUNT: usize = 64;

/// Build 4,096 distinct depth-one generic function instances without generic
/// recursion. A caller appends the expression that requests the next shared
/// instantiation, so count-limit tests can isolate that exact source site.
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

pub fn driver(): int {
    var sink: int = 0
"#,
    );
    // The 64 x 64 ordered pairs mint 4,096 distinct `prime<A, B>` rows.
    for left in 0..SHALLOW_SEED_TYPE_COUNT {
        for right in 0..SHALLOW_SEED_TYPE_COUNT {
            writeln!(
                source,
                "    sink = prime(N{left}(value: 0), N{right}(value: 0))"
            )
            .expect("write generated prime application");
        }
    }
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
        has_code(&diagnostics, "check.recursion"),
        "{diagnostics:#?}"
    );
}

/// A generic that recurses over an ever-growing type diverges monomorphization;
/// the instantiation bound (law 9) fails it with a typed `check.instantiation_limit`
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
        has_code(&diagnostics, "check.instantiation_limit"),
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

/// An Option-free fan-out leaves a previously queued safe follower behind the first
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

/// A depth refusal while resolving a generic function's return annotation is the
/// limit itself. It cannot be substituted with Unit and then reported as a second
/// return/body mismatch.
#[test]
fn depth_limit_in_a_generic_return_does_not_collapse_to_unit() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn deepen<T>(x: T): Grow<T> {
    return deepen(x)
}

pub fn driver(): int {
    const ignored = deepen(1)
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 7, 21);
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
    assert_eq!(diagnostic.code, "check.instantiation_limit");
    assert_eq!(diagnostic.file, "src/library.mw");
    assert_eq!((diagnostic.line, diagnostic.column), (7, 25));
}

/// A monomorphic signature cannot silently drop a parameter whose generic type
/// reaches the depth bound. The caller must not see a zero-parameter signature and
/// invent a wrong-arity diagnostic after the one limit refusal.
#[test]
fn depth_limit_does_not_drop_a_monomorphic_signature_parameter() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn take(value: Grow<int>): int {
    return 0
}

pub fn driver(): int {
    return take(0)
}
"#,
    );
    assert_one_located_limit(&diagnostics, 7, 16);
}

/// A monomorphic signature return that reaches the depth bound must remain a
/// refusal. Substituting Unit makes a value-position call report that the function
/// returns nothing, even though its declaration has a value return.
#[test]
fn depth_limit_does_not_substitute_unit_for_a_monomorphic_signature_return() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn make(): Grow<int> {
    unreachable("unreachable fixture")
}

pub fn driver(): int {
    const value = make()
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 7, 12);
}

/// Resolving a local's explicit annotation is a lowering boundary. A limit there
/// rejects the body directly; it is not an ordinary unsupported annotation layered
/// on top of the shared limit.
#[test]
fn depth_limit_in_an_annotated_binding_is_not_reclassified_as_unsupported() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(): int {
    const value: Grow<int> = 0
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 18);
}

/// A checked-result `const` resolves its annotation through `coerce_int_result`.
/// A recursive generic depth refusal is the one located limit, not contextual
/// Unsupported, and the unresolved binding cannot keep lowering the body.
#[test]
fn checked_const_annotation_preserves_a_limit_and_rejects_the_body() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(): int {
    const value: Grow<int> = checked 1 + 2
        on out_of_range return 0
    return value.next
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 18);
}

/// The mutable checked-result binding takes the same `coerce_int_result` path. Its
/// recursive annotation must reject the body with only the first real limit site.
#[test]
fn checked_var_annotation_preserves_a_limit_and_rejects_the_body() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(): int {
    var value: Grow<int> = checked 1 + 2
        on out_of_range return 0
    return value.next
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 16);
}

/// The `if const` annotation path currently ignores a failed `resolve_type`. A
/// shared limit must instead reject the body, without binding the optional's bare
/// type as though the requested annotation had resolved.
#[test]
fn depth_limit_in_an_if_const_annotation_is_not_ignored() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(): int {
    const maybe: int? = 1
    if const value: Grow<int> = maybe {
        return value
    } else {
        return 0
    }
}
"#,
    );
    assert_one_located_limit(&diagnostics, 9, 21);
}

/// The concrete-struct declaration pass owns its contextual unsupported diagnostic.
/// A depth refusal from a generic field type is the shared limit instead and must
/// not be reclassified while `struct_fields` drops the field.
#[test]
fn depth_limit_in_a_concrete_struct_field_is_not_reclassified_as_unsupported() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

struct Holder {
    value: Grow<int>
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 12);
}

/// The resource-record declaration pass has the same split: ordinary unsupported
/// fields retain their contextual diagnostic, but a generic depth refusal is only
/// the one shared located limit.
#[test]
fn depth_limit_in_a_resource_field_is_not_reclassified_as_unsupported() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

resource Holder {
    value: Grow<int>
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 8, 12);
}

/// An unkeyed group's materialized leaves resolve through `build_group_leaves`.
/// That declaration consumer must propagate Limit without adding its ordinary
/// unsupported-group-field diagnostic.
#[test]
fn depth_limit_in_a_group_leaf_is_not_reclassified_as_unsupported() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

resource Holder {
    details {
        value: Grow<int>
    }
}

pub fn driver(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 9, 16);
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
}

/// The sibling direct generic-enum constructor has the same refusal transfer: a
/// failed mint cannot leave the binding absent and let the following match invent
/// a secondary diagnostic.
#[test]
fn count_limit_at_a_direct_generic_enum_construction_rejects_the_body() {
    let mut source = shallow_function_reservation_fixture();

    let constructor_line = source.lines().count() as u32 + 1;
    assert_eq!(constructor_line, 4365, "generated source layout drifted");
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
}

/// Interpolation ordinarily accumulates independent part diagnostics. A shared
/// instantiation-limit refusal is terminal instead: once the first hole refuses,
/// no later hole may manufacture a secondary diagnostic from a rejected body.
#[test]
fn interpolation_stops_after_a_part_reaches_the_instantiation_limit() {
    let mut source = shallow_function_reservation_fixture();
    let constructor_line = source.lines().count() as u32 + 1;
    assert_eq!(constructor_line, 4365, "generated source layout drifted");
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
        &[("check.type", 4, 15), ("check.type", 4, 32)],
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
            ("check.instantiation_limit", 15, 17),
            ("check.unsupported", 7, 22),
            ("check.unsupported", 11, 22),
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
            ("check.instantiation_limit", 7, 17),
            ("check.unsupported", 11, 22),
            ("check.unsupported", 15, 22),
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
    assert_eq!(diagnostics[0].code, "check.unsupported");
    assert_eq!(diagnostics[0].file, "src/main.mw");
    assert_eq!((diagnostics[0].line, diagnostics[0].column), (7, 22));
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
    assert_diagnostic_sites(&diagnostics, &[("check.unsupported", 8, 18)]);
}

/// An unused generic template is checked against an isolated registry clone. Its
/// clone-local type limit and collection-payload refusal must both transfer to the
/// real diagnostic coordinator in canonical limit-before-payload order. The later
/// safe export only keeps the fixture free of an unrelated body diagnostic.
#[test]
fn proof_clone_transfers_its_limit_before_its_payload_diagnostic() {
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
            ("check.instantiation_limit", 11, 21),
            ("check.unsupported", 3, 22),
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

/// A generic template can reach the type-instantiation limit while resolving a
/// parameter before its first statement. The rejected signature stops that template
/// body immediately, so the statement cannot add a secondary name diagnostic.
#[test]
fn template_parameter_limit_stops_before_the_first_body_statement() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

fn inspect<T>(value: Grow<T>): int {
    return missing()
}

pub fn safe(): int {
    return 0
}
"#,
    );
    assert_one_located_limit(&diagnostics, 7, 22);
}

/// Once an `if` branch reaches the shared limit, no later branch is semantically
/// visited and no enclosing fallthrough diagnostic is manufactured.
#[test]
fn nested_if_limit_stops_later_branches_and_structural_diagnostics() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(flag: bool): int {
    if flag {
        const value: Grow<int> = 0
        return 1
    } else {
        return missing()
    }
}
"#,
    );
    assert_one_located_limit(&diagnostics, 9, 22);
}

/// The present branch of `if const` owns the same nested-block stop. Its absent
/// tail is not visited after a limit, and the enclosing value-return check stays
/// downstream of that stop.
#[test]
fn nested_if_const_limit_stops_the_absent_tail() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(): int {
    const maybe: int? = 1
    if const present = maybe {
        const value: Grow<int> = 0
        return 1
    } else {
        return missing()
    }
}
"#,
    );
    assert_one_located_limit(&diagnostics, 10, 22);
}

/// A limit in one enum arm stops arm dispatch before later arm bodies and before
/// the match owner's exhaustiveness/termination diagnostics.
#[test]
fn nested_match_arm_limit_stops_later_arms_and_match_diagnostics() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

enum Choice {
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
    );
    assert_one_located_limit(&diagnostics, 15, 26);
}

/// Checked-fault arms are nested control owners too. A limit in the first branch
/// of the handler stops its later branch and suppresses the handler-divergence
/// diagnostic that would otherwise be layered on the rejected body.
#[test]
fn nested_checked_arm_limit_stops_later_control_and_arm_diagnostics() {
    let diagnostics = compile_err(
        r#"module main

struct Grow<T> {
    next: Grow<List<T>>
}

pub fn driver(flag: bool): int {
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
    );
    assert_one_located_limit(&diagnostics, 11, 31);
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
    assert_diagnostic_sites(
        &diagnostics,
        &[("check.unsupported", 7, 17), ("check.unsupported", 11, 18)],
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
    assert_diagnostic_sites(&diagnostics, &[("check.unsupported", 12, 18)]);
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
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
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
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
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
        has_code(&diagnostics, "check.recursion"),
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
        has_code(&diagnostics, "check.recursion"),
        "{diagnostics:#?}"
    );
    let cycle = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "check.recursion")
        .expect("a recursion diagnostic");
    assert!(
        cycle.message.contains("Loop<int>"),
        "the cycle path must render `Loop<int>` in angle form: {}",
        cycle.message
    );
    assert!(
        cycle.message.contains("Box<Loop<int>>"),
        "the cycle path must render the nested `Box<Loop<int>>` in angle form: {}",
        cycle.message
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
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Map<string, List<int>>")),
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
    assert!(has_code(&diagnostics, "check.type"), "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Result<int, string>")),
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
        has_code(&diagnostics, "check.instantiation_limit"),
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
        has_code(&diagnostics, "check.name_conflict"),
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

/// A collection as a bare `some(...)` payload is refused at check time. The image
/// admits a scalar, record, or enum enum-payload leaf; a `List` is not one, so a
/// checker-clean program can never mint an image the verifier rejects at the Table
/// phase. The refusal is located at the constructed value.
#[test]
fn a_collection_some_payload_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

pub fn run(): int {
    const x = some(List(1, 2, 3))
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("`some` payload of `Option`")
                && d.message.contains("not a payload type")),
        "{diagnostics:#?}"
    );
}

/// The same rejection reaches an `Option<List<int>>` type annotation carrying no
/// constructor: the mint that resolves the annotation is where the collection
/// payload leaf is refused.
#[test]
fn a_collection_option_annotation_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

pub fn run(): int {
    const x: Option<List<int>> = none
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("not a payload type")),
        "{diagnostics:#?}"
    );
}

/// A `Result` whose `ok` payload monomorphizes to a `Map` is refused with the same
/// teaching diagnostic, naming the offending member.
#[test]
fn a_collection_result_payload_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

pub fn run(): Result<Map<int, int>, int> {
    return ok(Map())
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("`ok` payload of `Result`") && d.message.contains("`Map`")),
        "{diagnostics:#?}"
    );
}

/// A user generic enum instantiated with a collection type argument that lands in a
/// payload position is refused, exactly like the reserved generics.
#[test]
fn a_user_generic_enum_collection_argument_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

enum Box<T> {
    wrap(v: T)
}

pub fn run(): int {
    const x = Box::wrap(v: List(1, 2, 3))
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("`wrap` payload of `Box`")),
        "{diagnostics:#?}"
    );
}

/// A user generic enum whose template body wraps its parameter in a collection has a
/// collection payload leaf for every instantiation — even `E<int>` — and is refused
/// at the instantiation site.
#[test]
fn a_user_generic_enum_internal_collection_payload_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

enum E<T> {
    v(x: List<T>)
}

pub fn run(): int {
    const x = E::v(x: List(1))
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("`v` payload of `E`")),
        "{diagnostics:#?}"
    );
}

/// A collection buried under nested `Option` layers is still refused: the inner
/// `Option<List<int>>` mint carries the illegal leaf.
#[test]
fn a_nested_option_collection_payload_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

pub fn run(): int {
    const x = some(some(List(1, 2, 3)))
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("not a payload type")),
        "{diagnostics:#?}"
    );
}

/// A function parameter typed `Option<List<int>>` is refused during signature
/// resolution, so the collection payload leaf never reaches an image function type.
#[test]
fn a_collection_option_parameter_is_rejected() {
    let diagnostics = compile_err(
        r#"module main

pub fn takes(o: Option<List<int>>): int {
    return 0
}

pub fn run(): int {
    return 0
}
"#,
    );
    assert!(
        has_code(&diagnostics, "check.unsupported"),
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("not a payload type")),
        "{diagnostics:#?}"
    );
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
