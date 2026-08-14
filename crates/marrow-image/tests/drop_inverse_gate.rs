//! The transitive Drop-path audit: every function reachable from the armed
//! transaction guards' `Drop` implementations — the image draft's total inverse and
//! the compiler's generic-owner composite — is enumerated by name and its body
//! scanned, string/char literals blanked first, for the operations the total-inverse
//! law forbids: panics, assertions, `unwrap`/`expect`, range `drain`, slice
//! indexing, and allocation. A sentinel comment closes every audited body so a
//! truncated extraction fails loudly, and a plant probe proves the scanner sees each
//! forbidden token in real code while ignoring it inside a literal.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;
use source_projection::{is_ident_byte, without_cfg_test_items};

/// The audited drop-reachable set: `(source file, function name)`, each body closed
/// by its `// drop-path audit sentinel` line. The two `Drop` implementations are the
/// roots; the named functions are their complete reachable callees that mutate
/// state. (`Vec::truncate`, `Vec::pop`, `HashMap::remove`, and `BTreeMap::remove`
/// are the standard-library leaves the law admits.)
const DROP_REACHABLE: [(&str, &str); 9] = [
    ("marrow-image/src/draft.rs", "fn rollback_armed"),
    ("marrow-image/src/draft.rs", "fn drop"),
    ("marrow-image/src/site_plan.rs", "fn pop_suffix_to"),
    ("marrow-image/src/product.rs", "fn pop_suffix_to"),
    ("marrow-image/src/product.rs", "fn rewind_total"),
    ("marrow-image/src/product.rs", "fn truncate"),
    ("marrow-image/src/value_dag.rs", "fn truncate"),
    (
        "marrow-compile/src/types/mod.rs",
        "fn restore_generic_owners",
    ),
    ("marrow-compile/src/types/mod.rs", "fn rewind_to"),
];

/// The compiler composite guard's own `Drop`, audited with its sentinel like the
/// draft guard's.
const COMPOSITE_DROP: (&str, &str) = (
    "marrow-compile/src/types/owner_txn.rs",
    "impl Drop for GenericOwnerTxn",
);

/// The forbidden tokens: any of these inside an audited body breaks the total,
/// allocation-free, non-panicking inverse law — the panic family, range `drain`,
/// the allocation family, and the fallible-call family (`?` propagation has no
/// caller to propagate to inside a `Drop` inverse).
const FORBIDDEN: [&str; 30] = [
    "panic!",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "debug_assert",
    "unwrap(",
    "expect(",
    ".drain(",
    "unreachable!",
    "todo!",
    "unimplemented!",
    ".push(",
    ".push_front(",
    ".push_back(",
    ".insert(",
    ".extend(",
    ".extend_from_slice(",
    ".resize(",
    "Vec::with_capacity",
    ".to_string(",
    ".to_owned(",
    ".to_vec(",
    ".clone(",
    "format!",
    "vec!",
    "Box::new",
    "String::from",
    ".collect(",
    ".reserve(",
    ")?",
];

/// Whether `body` propagates a fallible call, including the whitespace-separated
/// spellings `)\n?` and `) ?` that a literal `")?"` scan cannot see. A `Drop` inverse has
/// no caller to propagate to, so any of them breaks the total-inverse law.
fn contains_fallible_propagation(body: &str) -> bool {
    let bytes = body.as_bytes();
    body.match_indices(')').any(|(at, _)| {
        bytes[at + 1..]
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|offset| bytes[at + 1 + offset] == b'?')
    })
}

/// Whether `body` contains a slice/array index expression — `[` directly after an
/// identifier character, `)`, or `]` — the panicking access the inverse law forbids.
/// Attribute markers (`#[`) and bare array types/literals do not match.
fn contains_index_expression(body: &str) -> bool {
    let bytes = body.as_bytes();
    body.match_indices('[').any(|(at, _)| {
        // Skip back over whitespace: `rows [0]` and `rows\n    [0]` index exactly as
        // `rows[0]` does, and a scan anchored on the immediately preceding byte sees
        // neither.
        let mut before = at;
        while before > 0 && bytes[before - 1].is_ascii_whitespace() {
            before -= 1;
        }
        before > 0
            && matches!(
                bytes[before - 1],
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b')' | b']'
            )
            // An attribute is not an index expression.
            && bytes[before - 1] != b'#'
    })
}

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// `code` with every string and char literal blanked, so a forbidden token inside a
/// message cannot hide a real one and a literal cannot trip the scan.
///
/// This crate's projection owner is the authority for the complete literal grammar —
/// ordinary, raw, byte, byte-raw, C, C-raw, and char forms. A scanner that blanks only
/// some of them silently disables itself for the rest: a forbidden token spelled inside
/// a `br"..."` is invisible to a scan that does not know the prefix, so a body could
/// carry `br".push("` beside a real `.push(` and the real one would still be found —
/// but a body carrying the token only inside such a literal would be *falsely* flagged,
/// and worse, a prefix the blanker mishandles desynchronizes the whole rest of the file.
fn without_literals(code: &str) -> String {
    // Delegate to the workspace's one projection owner rather than keeping a second,
    // narrower copy of the literal grammar here.
    source_projection::without_literals(code)
}

/// The body of `name` in `code`: from the declaration through its balanced closing
/// brace, asserting the sentinel line follows — the proof the extraction saw the
/// whole body rather than a truncated prefix.
fn audited_body(code: &str, raw: &str, name: &str, file: &str) -> String {
    let start = code
        .find(name)
        .unwrap_or_else(|| panic!("{file}: audited function `{name}` not found"));
    let open = code[start..]
        .find('{')
        .map(|at| start + at)
        .unwrap_or_else(|| panic!("{file}: `{name}` has no body"));
    let mut depth = 0usize;
    let mut end = None;
    for (at, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + at + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.unwrap_or_else(|| panic!("{file}: `{name}` body is unbalanced"));
    // The sentinel may sit after the enclosing impl's own closing brace when the
    // audited function is the impl's last item.
    // The sentinel is a comment, and the projection blanks comments, so it is asserted
    // against the raw source at the same offset the projection matched.
    let mut tail = raw[end..].trim_start();
    while let Some(rest) = tail.strip_prefix('}') {
        tail = rest.trim_start();
    }
    assert!(
        tail.starts_with("// drop-path audit sentinel: end of"),
        "{file}: `{name}` is not sentinel-terminated — the audited region may be truncated",
    );
    code[start..end].to_string()
}

/// The counting-run bodies: the capped measurement walk and its arithmetic section
/// counters, each closed by a `// count-path audit sentinel` line. The shared
/// section writers they drive are streaming codecs over caller-held sinks; the one
/// sanctioned pre-verdict allocation (the adjudicated DURABLE expansion worklist)
/// lives in the traversal owner outside these bodies.
const COUNT_PATH: [(&str, &str); 4] = [
    ("marrow-image/src/measure.rs", "fn measure"),
    ("marrow-image/src/measure.rs", "fn count_section_body"),
    ("marrow-image/src/measure.rs", "fn count_functions"),
    ("marrow-image/src/measure.rs", "fn count_spans"),
];

/// The allocation-class tokens the counting run must not spell: the N+1 decisive
/// refusal and the fitting count alike run with zero heap events of their own.
const COUNT_FORBIDDEN: [&str; 11] = [
    "Vec::with_capacity",
    "Vec::new",
    "vec!",
    "format!",
    "String::from",
    ".to_string(",
    ".to_owned(",
    ".to_vec(",
    "Box::new",
    ".collect(",
    "with_capacity(",
];

/// The body of `name` in `code`, terminated by the count-path sentinel.
fn counted_body(code: &str, raw: &str, name: &str, file: &str) -> String {
    let start = code
        .find(name)
        .unwrap_or_else(|| panic!("{file}: counted function `{name}` not found"));
    let sentinel = raw[start..]
        .find("// count-path audit sentinel: end of")
        .unwrap_or_else(|| panic!("{file}: `{name}` is not count-sentinel-terminated"));
    code[start..start + sentinel].to_string()
}

/// Every counting-run body is free of the allocation-class tokens — **transitively**.
///
/// A lexical scan of four named bodies is weaker than its subject: adding a call to an
/// existing allocating helper introduces no forbidden token at the call site, so the
/// scan stays green while the counting run allocates. The audit therefore follows the
/// call graph out of those four roots and scans every body it reaches, and it asserts
/// the reached set against a census so a newly reached callee is conspicuous rather than
/// silently absorbed.
///
/// (The observed-zero-allocation run itself remains a recorded open item: a counting
/// global allocator needs `unsafe`, which the workspace forbids.)
#[test]
fn the_counting_run_spells_no_allocation() {
    let definitions = crate_function_bodies();
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut frontier: Vec<String> = Vec::new();

    for (file, name) in COUNT_PATH {
        let raw = fs::read_to_string(workspace_file(file))
            .unwrap_or_else(|_| panic!("read {file} for the count-path audit"));
        let code = without_literals(&raw);
        let body = counted_body(&code, &raw, name, file);
        assert_count_clean(&body, file, name);
        frontier.push(body);
    }

    // Resolution is by name, which is sound only when a name has exactly one definition
    // in the crate. A name with several (`members`, `len`, `new`) does not identify the
    // code that actually runs, so scanning all of them would report an allocation in a
    // body the counting run never enters. Uniquely resolved callees are followed and
    // scanned; ambiguous ones are recorded in a census instead, so a newly ambiguous
    // reach is visible rather than silently dropped.
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    let mut cleared_by_exhaustion = 0usize;
    while let Some(body) = frontier.pop() {
        for callee in called_names(&body) {
            if !reached.insert(callee.clone()) {
                continue;
            }
            let Some(bodies) = definitions.get(&callee) else {
                continue;
            };
            if bodies.len() > 1 {
                // Ambiguous by name, but not therefore unresolved. Which definition runs
                // is unknowable to a text scan; whether *any* of them could allocate is
                // not. Scanning every definition resolves the name by exhaustion: if all
                // of them are clean, the call is clean whichever one runs. Only a name
                // with an unclean definition is left as residue, and that residue is what
                // has to be adjudicated by hand.
                //
                // The definitions are not expanded. Following the callees of every
                // candidate would walk the audit into code this path never enters — the
                // `encode` case below demonstrates it on this crate — so resolution by
                // exhaustion is stated at the body and not claimed transitively.
                if bodies.iter().all(|(_, candidate)| count_clean(candidate)) {
                    cleared_by_exhaustion += 1;
                } else {
                    ambiguous.insert(callee.clone());
                }
                continue;
            }
            for (file, callee_body) in bodies {
                assert_count_clean(callee_body, file, &callee);
                frontier.push(callee_body.clone());
            }
        }
    }
    assert!(
        cleared_by_exhaustion > 0,
        "no ambiguous name was resolved by exhaustion, so the walk is not scanning the \
         candidate definitions it cannot choose between",
    );
    let recorded: BTreeSet<String> = UNRESOLVED_COUNT_PATH_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        ambiguous, recorded,
        "the residue moved: these are the counting-path callees this crate defines under \
         more than one name where at least one candidate definition is not count-clean, so \
         exhaustion does not resolve them. A new one must be adjudicated by hand.",
    );

    // The walk really followed the graph out of the roots rather than stopping at them.
    assert!(
        reached.len() > 20,
        "the counting-run walk reached only {} callees, so it is not following the call \
         graph out of the four roots",
        reached.len(),
    );
    assert!(
        reached.contains("expand"),
        "the walk reaches `expand`, the DURABLE traversal the roots drive; if it does \
         not, the graph is not being followed",
    );
}

/// The counting-path callees this crate defines under more than one name **and** whose
/// candidate definitions are not all allocation-free, so the walk cannot clear them.
///
/// This list is a residue, not a skip list, and it is three names rather than thirty-eight.
/// A name with several definitions is not thereby unresolved: which definition runs is
/// unknowable to a text scan, but whether *any* of them could allocate is not. The walk
/// scans every candidate, and a name whose candidates are all clean is resolved by
/// exhaustion — clean whichever one runs. Only a name with an unclean candidate survives
/// here, and each survivor is adjudicated by hand.
///
/// Two tightenings got the count down and both are sound. Test-module definitions are no
/// longer counted as candidates: they are not code the counting run can enter, and
/// including them made names ambiguous that production defines exactly once. And exhaustion
/// replaced the skip.
///
/// **The residue is stated at the body, not transitively.** A resolved-by-exhaustion name's
/// callees are deliberately not expanded: following the callees of every candidate walks the
/// audit into code this path never enters, and
/// [`the_same_file_tightening_binds_a_method_call_to_the_wrong_definition`] demonstrates the
/// same class of false reach on this crate — `encode.rs` calls `field.ty.encode(sink)`, a
/// method of `ImageType` declared in `ty.rs`, while `encode.rs` also declares exactly one
/// `fn encode`, the whole emission driver. Binding by proximity walks the counting audit
/// straight into emission. Resolution by exhaustion at the body is the tighter answer
/// available to a scanner without types, and this is why.
const UNRESOLVED_COUNT_PATH_NAMES: &[&str] = &[
    // `new` is the clearest case and stands for the other two: several of this crate's
    // `new` definitions build owners, so exhaustion cannot clear the name — a scan that
    // cannot say which `new` runs cannot say the call is allocation-free.
    "members",
    "members_of",
    "new",
];

/// The adjudicated allocations on the counting path: `(file, function, token)`.
///
/// The counting run's zero-heap posture has exactly one sanctioned exception — the
/// DURABLE expansion worklist, which the traversal owner allocates once so that a shared
/// value graph is walked without materializing its expansion. The previous gate stated
/// this exception in prose while scanning only four bodies that did not contain it, so
/// nothing checked that it stayed the only one. It is now an entry: a second allocation
/// anywhere on the reachable counting path fails until it is adjudicated here.
const SANCTIONED_COUNT_PATH_ALLOCATIONS: &[(&str, &str, &str)] =
    &[("value_dag.rs", "expand", "vec!")];

/// One body carries no allocation-class token beyond its adjudicated exceptions.
fn assert_count_clean(body: &str, file: &str, name: &str) {
    for token in COUNT_FORBIDDEN {
        if SANCTIONED_COUNT_PATH_ALLOCATIONS.contains(&(file, name, token)) {
            continue;
        }
        assert!(
            !body.contains(token),
            "{file}: `{name}` contains allocation-class `{token}` on the counting path",
        );
    }
}

/// Whether `body` is free of every allocation-class token, with no sanction applied.
///
/// Resolution by exhaustion asks whether a candidate could allocate at all, so it reads
/// the unsanctioned answer: a sanction is granted to a named site on the counting path,
/// and a candidate this walk cannot even name has not been granted one.
fn count_clean(body: &str) -> bool {
    COUNT_FORBIDDEN.iter().all(|token| !body.contains(token))
}

/// Every `fn name` definition in this crate's production source, with its brace-matched
/// body, so the counting walk can resolve a call to the code it runs.
fn crate_function_bodies() -> std::collections::BTreeMap<String, Vec<(String, String)>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    let mut definitions: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for path in files {
        let name = path
            .display()
            .to_string()
            .split_once("src/")
            .expect("a src path")
            .1
            .to_string();
        // Test-module definitions are not code the counting run can enter, and counting
        // them as candidates made names ambiguous that production defines exactly once.
        let code = without_cfg_test_items(&without_literals(
            &fs::read_to_string(&path).expect("read source"),
        ));
        for (fn_name, body) in function_bodies(&code) {
            definitions
                .entry(fn_name)
                .or_default()
                .push((name.clone(), body));
        }
    }
    assert!(
        !definitions.is_empty(),
        "the source tree yielded definitions"
    );
    definitions
}

/// Every `fn name` definition in `code`, paired with its brace-matched body.
fn function_bodies(code: &str) -> Vec<(String, String)> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(hit) = code[at..].find("fn ") {
        let start = at + hit;
        at = start + 3;
        if start > 0 && is_ident_byte(bytes[start - 1]) {
            continue;
        }
        let rest = &code[start + 3..];
        let name_len = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_len == 0 {
            continue;
        }
        let name = rest[..name_len].to_string();
        let Some(open) = code[start..].find('{').map(|n| start + n) else {
            continue;
        };
        let mut depth = 0i32;
        let mut end = None;
        for (offset, byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end) = end {
            out.push((name, code[open..=end].to_string()));
        }
    }
    out
}

/// Every identifier `body` calls.
fn called_names(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut names = Vec::new();
    for (at, _) in body.match_indices('(') {
        let mut start = at;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        if start == at {
            continue;
        }
        let name = &body[start..at];
        if KEYWORDS.contains(&name) || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        names.push(name.to_string());
    }
    names.sort();
    names.dedup();
    names
}

/// The count-path scanner sees a planted allocation token and refuses a body with no
/// sentinel.
#[test]
fn the_count_path_scanner_detects_a_planted_allocation() {
    for token in COUNT_FORBIDDEN {
        let planted = format!(
            "fn probe() {{ let _ = {token}; }}\n// count-path audit sentinel: end of probe\n"
        );
        let body = counted_body(&without_literals(&planted), &planted, "fn probe", "planted");
        assert!(
            body.contains(token),
            "the count-path scanner failed to see planted `{token}`",
        );
    }
    let unterminated = "fn probe() { }\n";
    let outcome = std::panic::catch_unwind(|| {
        counted_body(
            &without_literals(unterminated),
            unterminated,
            "fn probe",
            "planted",
        )
    });
    assert!(
        outcome.is_err(),
        "an unterminated counted body must fail the sentinel check loudly",
    );
}

/// Every audited drop-reachable body is free of the forbidden operations.
#[test]
fn the_armed_inverse_paths_are_total_and_allocation_free() {
    let mut audited = 0usize;
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (file, name) in DROP_REACHABLE.into_iter().chain([COMPOSITE_DROP]) {
        let raw = fs::read_to_string(workspace_file(file))
            .unwrap_or_else(|_| panic!("read {file} for the drop-path audit"));
        let code = without_literals(&raw);
        // Audit every same-named function in the file (`pop_suffix_to` appears once
        // per owner; `fn drop` is matched at each audited impl's sentinel).
        let mut from = 0usize;
        while let Some(at) = code[from..].find(name) {
            let region = &code[from + at..];
            // Only audit occurrences that are sentinel-terminated definitions.
            let body = audited_body(region, &raw[from + at..], name, file);
            for token in FORBIDDEN {
                assert!(
                    !body.contains(token),
                    "{file}: `{name}` contains forbidden `{token}` on the Drop path",
                );
            }
            assert!(
                !contains_index_expression(&body),
                "{file}: `{name}` contains a panicking index expression on the Drop path",
            );
            assert!(
                !contains_fallible_propagation(&body),
                "{file}: `{name}` propagates a fallible call on the Drop path, which has \
                 no caller to propagate to",
            );
            audited += 1;
            seen.insert((file, name));
            from += at + body.len();
        }
    }
    // Fail loud *per subject*: a total count can be met by one file contributing two
    // bodies while another contributes none, so the audit asserts that every declared
    // subject was found by name.
    for (file, name) in DROP_REACHABLE.into_iter().chain([COMPOSITE_DROP]) {
        assert!(
            seen.contains(&(file, name)),
            "the drop-path audit lost the subject `{name}` in {file}; it must be found and \
             scanned, not merely counted",
        );
    }
    assert_eq!(
        seen.len(),
        DROP_REACHABLE.len() + 1,
        "every declared drop-reachable subject is audited exactly once by name",
    );
    assert!(audited >= seen.len(), "found {audited} bodies");
}

/// The plant probe: the scanner must flag each forbidden token in real code and must
/// not flag one hidden inside a string literal (including a raw literal), and the
/// sentinel check must refuse an unterminated body.
#[test]
fn the_drop_path_scanner_detects_a_planted_violation() {
    for token in FORBIDDEN {
        let planted = format!(
            "fn probe() {{ let _ = {token}; }}\n// drop-path audit sentinel: end of probe\n"
        );
        let body = audited_body(&without_literals(&planted), &planted, "fn probe", "planted");
        assert!(
            body.contains(token),
            "the scanner failed to see planted `{token}` in code",
        );
        // Every literal form, not only the two the previous plant covered: a prefixed
        // raw form the blanker does not know desynchronizes the rest of the scan.
        for spelling in [
            format!("\"{token}\""),
            format!("r\"{token}\""),
            format!("r#\"{token}\"#"),
            format!("b\"{token}\""),
            format!("br\"{token}\""),
            format!("br#\"{token}\"#"),
            format!("c\"{token}\""),
            format!("cr\"{token}\""),
            format!("cr#\"{token}\"#"),
        ] {
            let literal = format!(
                "fn probe() {{ let _ = {spelling}; }}\n// drop-path audit sentinel: end of probe\n"
            );
            let body = audited_body(&without_literals(&literal), &literal, "fn probe", "planted");
            assert!(
                !body.contains(token),
                "the scanner saw `{token}` inside a blanked {spelling} literal",
            );
        }
        // A char literal must not desynchronize the blanker either.
        let with_char = format!(
            "fn probe() {{ let _ = '\\''; let _ = \"{token}\"; }}\n// drop-path audit sentinel: end of probe\n"
        );
        let body = audited_body(
            &without_literals(&with_char),
            &with_char,
            "fn probe",
            "planted",
        );
        assert!(
            !body.contains(token),
            "a char literal desynchronized the blanker before `{token}`",
        );
    }
    let indexed = "fn probe() { let _ = rows[0]; }\n// drop-path audit sentinel: end of probe\n";
    assert!(
        contains_index_expression(&audited_body(
            &without_literals(indexed),
            indexed,
            "fn probe",
            "planted"
        )),
        "the scanner failed to see a planted index expression",
    );
    let attribute = "fn probe() { #[allow(unused)] let x: [u8; 2] = [0, 1]; }\n// drop-path audit sentinel: end of probe\n";
    assert!(
        !contains_index_expression(&audited_body(
            &without_literals(attribute),
            attribute,
            "fn probe",
            "planted"
        )),
        "an attribute or array literal is not an index expression",
    );
    let unterminated = "fn probe() { }\nfn other() {}\n";
    let outcome = std::panic::catch_unwind(|| {
        audited_body(
            &without_literals(unterminated),
            unterminated,
            "fn probe",
            "planted",
        )
    });
    assert!(
        outcome.is_err(),
        "an unterminated audited body must fail the sentinel check loudly",
    );
}

/// Every call expression in an audited Drop-path body resolves to something known.
///
/// This is the completeness half of the audit, and it replaces a closure keyed on call
/// names. That closure over-approximated badly — `new`, `len`, and `members` each resolve to
/// many definitions, so following all of them reported allocations in bodies the Drop path
/// never enters — and it was backed out twice for that reason.
///
/// Inverting the burden fixes it. Rather than resolving the whole crate, every call in an
/// audited body must land in one of exactly two places: an allowlisted call whose safety on
/// this path was reviewed once, or another audited body, which is scanned by the same
/// rules. Anything else is UNRESOLVED and fails loudly. A newly reachable callee therefore
/// cannot be absorbed silently — it either joins the allowlist with a reason or joins the
/// audit.
///
/// **The allowlist is keyed on the whole call path, not on the bare name.** A bare name
/// admits every call that happens to end in it: allowlisting `remove` for
/// `HashMap::remove` also cleared any other `remove` a Drop body might grow, and
/// allowlisting `take` cleared a free `take` as readily as `Option::take`. The receiver is
/// the part that carries the safety argument, so it is the part that is recorded.
const RESOLVED_CALL_PATHS: &[&str] = &[
    // `Option::as_mut` and the `RefCell::get_mut` family: an exclusive borrow of an owner
    // the guard already holds. No heap event, no panic, total on any state.
    "as_mut",
    // `Option::take` on the composite guard's own owners: taking each out is what makes
    // every armed inverse run exactly once, on the drop path as on the explicit one.
    "self.draft.take",
    "self.inverse.take",
    "draft.enums.get_mut",
    "draft.enums_fill.get_mut",
    "draft.types.get_mut",
    "draft.types_fill.get_mut",
    "self.collection_index.get_mut",
    "self.collections.get_mut",
    "self.generics.get_mut",
    "self.row_directory.get_mut",
    // Length and last-element reads on the owners the inverse rewinds. Total on an empty
    // owner — `last` answers `None` rather than panicking.
    "colls.len",
    "draft.consts.last",
    "draft.consts.len",
    "draft.strings.last",
    "draft.strings.len",
    "generics.fn_insts.len",
    "generics.type_insts.len",
    "self.store.len",
    // Identity and index readers on a value the inverse already owns: field projections
    // that allocate nothing.
    "discarded.claim.identity",
    "id.index",
    // In-place shrink. `pop` on an empty owner answers `None`, and every `pop` here is
    // guarded by the matching length read above; none of them can allocate.
    "colls.pop",
    "draft.consts.pop",
    "draft.strings.pop",
    "generics.fn_insts.pop",
    "generics.type_insts.pop",
    "self.journal.fills.pop",
    "self.rows.last",
    "self.rows.len",
    "self.rows.pop",
    // Keyed removal from the reuse indexes the rewound rows were registered in. A map
    // removal frees rather than allocates, and a missing key answers `None`.
    "draft.const_index.remove",
    "draft.string_index.remove",
    "generics.fn_index.remove",
    "generics.type_index.remove",
    "index.remove",
    "self.by_identity.remove",
    "self.retained.remove",
    // Whole-owner resets and the retain that drops interned rows above the restored
    // length. `clear` and `retain` free; neither can grow an owner.
    "generics.fill_failures.clear",
    "generics.fill_rows.clear",
    "generics.fill_stack.clear",
    "self.interned.retain",
    // Constructors and the bare standard-library leaves reached without a receiver
    // spelling. `truncate` past the length is a no-op rather than a panic.
    "Some",
    "clear",
    "first",
    "identity",
    "index",
    "last",
    "len",
    "pop",
    "remove",
    "retain",
    "truncate",
];

#[test]
fn every_drop_path_call_resolves_to_a_primitive_or_another_audited_body() {
    let audited: BTreeSet<&str> = DROP_REACHABLE
        .iter()
        .map(|(_, name)| name.trim_start_matches("fn "))
        .collect();

    let mut resolved = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    // The composite guard's own `Drop` is audited by the scan beside this one, so it is
    // audited by this one too: a resolver that reads a smaller set than the scanner leaves
    // the difference unresolved while reporting a clean result.
    for (file, name) in DROP_REACHABLE.into_iter().chain([COMPOSITE_DROP]) {
        let raw = fs::read_to_string(workspace_file(file))
            .unwrap_or_else(|_| panic!("read {file} for the drop-path resolution audit"));
        let code = without_literals(&raw);
        let body = audited_body(&code, &raw, name, file);
        for call in called_paths_in(&body) {
            let known = RESOLVED_CALL_PATHS.contains(&call.as_str())
                || audited.contains(final_segment(&call));
            if !known {
                unresolved.push(format!("{file}: `{name}` calls `{call}`"));
            }
            resolved += 1;
        }
    }
    assert!(
        unresolved.is_empty(),
        "these Drop-path calls resolve to neither an allowlisted call path nor another \
         audited body. Resolve each: add it to the audit if it is reachable code of the \
         inverse, or to the allowlist with the reason it is non-allocating, non-panicking, \
         and total here.\n{}",
        unresolved.join("\n"),
    );
    assert!(
        resolved > 20,
        "the resolution audit examined only {resolved} calls, so it is not reading the bodies",
    );
}

/// The call-path scanner distinguishes receivers that a bare-name scanner collapsed.
#[test]
fn the_call_path_scanner_keeps_the_receiver() {
    let body = "{ self.journal.pop(); other.pop(); Vec::pop(v); pop(); }";
    let paths = called_paths_in(body);
    assert!(
        paths.contains("self.journal.pop"),
        "the receiver is part of the resolved call path: {paths:?}",
    );
    assert!(
        paths.contains("Vec::pop") && paths.contains("other.pop") && paths.contains("pop"),
        "each distinct receiver is its own call path rather than one shared bare name: \
         {paths:?}",
    );
    assert_eq!(
        paths.len(),
        4,
        "four distinct receivers are four call paths, not one: {paths:?}",
    );
    assert_eq!(final_segment("self.journal.pop"), "pop");
    assert_eq!(final_segment("Vec::pop"), "pop");
    assert_eq!(final_segment("pop"), "pop");
}

/// The control-flow keywords a backward walk from `(` can land on, which are not calls.
const KEYWORDS: [&str; 8] = ["if", "while", "for", "match", "fn", "return", "let", "in"];

/// The final segment of a call path — the name a definition would carry.
fn final_segment(path: &str) -> &str {
    let after_colons = path.rsplit("::").next().unwrap_or(path);
    after_colons.rsplit('.').next().unwrap_or(after_colons)
}

/// Every call path `body` calls, receiver included.
///
/// The walk back from an opening parenthesis takes `.` and `::` as part of the path rather
/// than as boundaries, so `self.journal.pop()`, `Vec::pop(v)`, and a free `pop()` are three
/// distinct call paths instead of one name `pop`.
fn called_paths_in(body: &str) -> BTreeSet<String> {
    let bytes = body.as_bytes();
    let mut paths = BTreeSet::new();
    for (at, _) in body.match_indices('(') {
        let mut start = at;
        while start > 0 {
            let byte = bytes[start - 1];
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
                start -= 1;
            } else if byte == b':' && start >= 2 && bytes[start - 2] == b':' {
                start -= 2;
            } else {
                break;
            }
        }
        let path = body[start..at].trim_matches('.');
        if path.is_empty() {
            continue;
        }
        let head = final_segment(path);
        if KEYWORDS.contains(&head) || head.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        paths.insert(path.to_string());
    }
    paths
}

/// The plant probe for the resolver: an unaudited callee is reported rather than stepped
/// over, and a receiver-qualified spelling of an allowlisted name is reported too.
#[test]
fn the_drop_path_resolver_reports_a_planted_unresolved_call() {
    let planted = "fn probe() { let _ = some_unaudited_helper(x); }\n\
                   // drop-path audit sentinel: end of probe\n";
    let body = audited_body(&without_literals(planted), planted, "fn probe", "planted");
    let calls = called_paths_in(&body);
    assert!(
        calls.contains("some_unaudited_helper"),
        "the resolver sees the call at all: {calls:?}",
    );
    assert!(
        !RESOLVED_CALL_PATHS.contains(&"some_unaudited_helper"),
        "an unknown callee is not silently allowlisted, so the audit above fails on it",
    );
    // The bare-name allowlist would have cleared this; the call-path allowlist does not,
    // because the receiver is what carries the safety argument.
    assert!(
        RESOLVED_CALL_PATHS.contains(&"pop"),
        "the bare primitive is allowlisted, so this contrast has a live subject",
    );
    assert!(
        !RESOLVED_CALL_PATHS.contains(&"some_unaudited_owner.pop"),
        "a new receiver for an allowlisted name is a new call path, not an absorbed one",
    );
}

/// The residue is unresolvable rather than merely unresolved: the obvious tightening binds
/// a method call to the wrong definition, on this crate's own source.
///
/// A scanner sees `receiver.name(args)` and `name(args)` as the same shape. Preferring a
/// definition beside the caller therefore resolves a method call by where its *caller*
/// lives, which has nothing to do with the receiver's type. This is the check that turns
/// "a lexical scanner cannot do better" from a claim in a doc comment into a fact about
/// this tree, so a later pass does not spend the tightening again and ship the false reach.
#[test]
fn the_same_file_tightening_binds_a_method_call_to_the_wrong_definition() {
    let definitions = crate_function_bodies();

    // `encode` is declared in several files, so a crate-wide lookup correctly refuses it.
    let encodes = definitions
        .get("encode")
        .expect("this crate declares `encode`");
    assert!(
        encodes.len() > 1,
        "`encode` is ambiguous crate-wide, which is why it is censused",
    );

    // Exactly one of them is in `encode.rs`, so a file-preferring resolver would pick it.
    let in_encode: Vec<&(String, String)> = encodes
        .iter()
        .filter(|(file, _)| file == "encode.rs")
        .collect();
    assert_eq!(
        in_encode.len(),
        1,
        "`encode.rs` declares exactly one `fn encode`, so a file-preferring resolver has \
         a unique — and wrong — answer for every `encode(` call that file spells",
    );

    // And that one definition is the emission driver, which allocates and calls the
    // emission the counting run must never be walked into.
    let (_, driver) = in_encode[0];
    assert!(
        driver.contains("emit_image("),
        "the definition a file-preferring resolver would bind to is the emission driver",
    );

    // The call it would be bound to is a method on a type declared elsewhere.
    let calls = fs::read_to_string(workspace_file("marrow-image/src/encode.rs"))
        .expect("read the emission owner");
    assert!(
        without_literals(&calls).contains(".ty.encode(sink)"),
        "`encode.rs` calls `encode` as a method of a type it does not declare",
    );
}
