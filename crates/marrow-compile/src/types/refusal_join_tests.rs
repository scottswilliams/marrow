//! E5 — the refusal lattice, pinned in every argument combination.
//!
//! `join` folds refusals for sub-parts of one annotation and for one provisional
//! instantiation row. Getting an arm wrong is silent: it would report the wrong
//! cause at a use site, or hide a genuine absence behind a refused sibling.

use super::*;
use crate::decl::{DeclarationLedger, DeclarationNamespace, DeclarationOccurrence, refuse_covered};

/// Two distinct refusal handles, minted by a real ledger so their namespace
/// tags and indexes are the ones production would produce.
fn handles() -> (ResolveRefusal, ResolveRefusal) {
    let mut ledger: DeclarationLedger<String, ()> = DeclarationLedger::new(
        DeclarationNamespace::NamedType,
        DeclarationBudget::default(),
    );
    let mut refusal = |name: &str| {
        let (identity, _) = FileIdentity::validate("src/main.mw").expect("a valid source path");
        let declared = DeclarationSite {
            name,
            file: &identity,
            at: FileRef::admitted(0),
            span: SourceSpan {
                start_byte: 0,
                end_byte: 1,
                line: 1,
                column: 1,
            },
        };
        ledger
            .declare(
                name.to_string(),
                DeclarationOccurrence::Refused(refuse_covered(declared, Code::CheckType.as_str())),
            )
            .expect("within budget");
        match ledger.lookup(&name.to_string()) {
            Ok(Binding::Refused(id, _)) => ResolveRefusal::RefusedDeclaration(id),
            _ => panic!("expected a refusal"),
        }
    };
    let first = refusal("A");
    let second = refusal("B");
    (first, second)
}

#[test]
fn the_lattice_holds_in_all_nine_argument_combinations() {
    let (one, two) = handles();
    let limit = ResolveRefusal::Limit;
    let gap = ResolveRefusal::Unsupported;

    // A terminal shared limit dominates, in either position.
    assert_eq!(limit.join(limit), limit);
    assert_eq!(limit.join(gap), limit);
    assert_eq!(limit.join(one), limit);
    assert_eq!(gap.join(limit), limit);
    assert_eq!(one.join(limit), limit);

    // A genuine absence dominates a refused declaration: a real gap must never
    // be reported as some refused sibling's fault.
    assert_eq!(gap.join(gap), gap);
    assert_eq!(gap.join(one), gap);
    assert_eq!(one.join(gap), gap);

    // Two refusals survive as one cause only when they are the same
    // declaration; two different ones have no single cause to steer to.
    assert_eq!(one.join(one), one);
    assert_eq!(one.join(two), gap);
    assert_eq!(two.join(one), gap);
}

/// The same-handle rule is keyed on the namespace tag too, so equal indexes in
/// two ledgers do not collapse into one steer.
#[test]
fn handles_from_two_namespaces_never_merge() {
    let mut types: DeclarationLedger<String, ()> = DeclarationLedger::new(
        DeclarationNamespace::NamedType,
        DeclarationBudget::default(),
    );
    let mut roots: DeclarationLedger<String, ()> = DeclarationLedger::new(
        DeclarationNamespace::DurableRoot,
        DeclarationBudget::default(),
    );
    let (identity, _) = FileIdentity::validate("src/main.mw").expect("a valid source path");
    let declared = DeclarationSite {
        name: "x",
        file: &identity,
        at: FileRef::admitted(0),
        span: SourceSpan {
            start_byte: 0,
            end_byte: 1,
            line: 1,
            column: 1,
        },
    };
    let first = |ledger: &mut DeclarationLedger<String, ()>| {
        ledger
            .declare(
                "x".to_string(),
                DeclarationOccurrence::Refused(refuse_covered(declared, Code::CheckType.as_str())),
            )
            .expect("within budget");
        match ledger.lookup(&"x".to_string()) {
            Ok(Binding::Refused(id, _)) => ResolveRefusal::RefusedDeclaration(id),
            _ => panic!("expected a refusal"),
        }
    };
    let from_types = first(&mut types);
    let from_roots = first(&mut roots);
    assert_ne!(from_types, from_roots);
    assert_eq!(from_types.join(from_roots), ResolveRefusal::Unsupported);
}
