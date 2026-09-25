//! Per-owner member reads against the whole member ledger.
//!
//! A record's field list and its refused members are read as one owner's contiguous
//! run of the ledger index. These tests declare owners whose keys would interleave
//! under any order that does not compare the whole owner first — a group anchor
//! `R.g`, names with `R` as a prefix, and the same spelling in a dependency tree —
//! and hold every owner's reads to what the whole ledger says about that owner.

use super::test_fixtures::test_registry;
use super::*;

use crate::decl::{DeclarationOccurrence, DeclarationSite, refuse};
use crate::diag::DiagnosticCollector;
use marrow_project::DependencyAlias;

#[derive(Clone, Copy)]
enum Step {
    /// Accepted; the flag rides on `FieldInfo::required` so a second acceptance of
    /// one key is distinguishable from its first.
    Accept(bool),
    Refuse,
}

fn refusal(member: &str) -> DeclarationRefusalSummary {
    let file = crate::test_file("src/main.mw").clone();
    refuse(
        &mut DiagnosticCollector::new(),
        DeclarationSite {
            name: member,
            file: &file,
            at: crate::analysis::FileRef::admitted(0),
            span: SourceSpan::default(),
        },
        marrow_codes::Code::CheckType,
        "refused".to_string(),
    )
}

#[test]
fn each_owner_reads_exactly_its_own_members() {
    use Step::{Accept, Refuse};
    let root = SourceOrigin::Root;
    let dependency =
        SourceOrigin::Dependency(DependencyAlias::parse("dep").expect("a valid alias"));
    let record = ScopedName::new(&root, "R");
    let group = record.below("g");
    let prefixed = ScopedName::new(&root, "Ra");
    let dashed = ScopedName::new(&root, "R-x");
    let foreign = ScopedName::new(&dependency, "R");
    let owners = [&record, &group, &prefixed, &dashed, &foreign];

    let script: Vec<(&ScopedName, &str, Step)> = vec![
        (&record, "title", Accept(true)),
        (&group, "year", Accept(false)),
        (&foreign, "title", Accept(false)),
        (&prefixed, "pages", Accept(false)),
        (&record, "zz", Refuse),
        (&dashed, "m", Accept(true)),
        (&record, "author", Accept(false)),
        (&group, "edition", Accept(true)),
        (&record, "bb", Refuse),
        (&foreign, "about", Accept(true)),
        (&record, "title", Accept(false)),
        (&prefixed, "alpha", Accept(true)),
        (&dashed, "b", Accept(false)),
        (&record, "middle", Accept(true)),
    ];

    let mut registry = test_registry(vec![]);
    for (owner, member, step) in &script {
        let occurrence = match step {
            Accept(required) => DeclarationOccurrence::Accepted(FieldInfo {
                name: member.to_string(),
                ty: GArg::Scalar(crate::scalar::ScalarType::Int),
                required: *required,
            }),
            Refuse => DeclarationOccurrence::Refused(refusal(member)),
        };
        registry
            .members
            .declare(MemberKey::new(owner, member), occurrence)
            .expect("within budget");
    }

    for owner in owners {
        let mut first: Vec<(&str, Step)> = Vec::new();
        for (_, member, step) in script.iter().filter(|(o, ..)| *o == owner) {
            if !first.iter().any(|(seen, _)| seen == member) {
                first.push((member, *step));
            }
        }
        let accepted_oracle: Vec<(&str, bool)> = first
            .iter()
            .filter_map(|(member, step)| match step {
                Accept(required) => Some((*member, *required)),
                Refuse => None,
            })
            .collect();
        let refused_oracle: Vec<&str> = registry
            .members
            .refused()
            .filter(|(key, _)| key.owner == *owner)
            .map(|(key, _)| key.member.as_str())
            .collect();

        let accepted = registry.accepted_members(owner).expect("a coherent ledger");
        let accepted: Vec<(&str, bool)> = accepted
            .iter()
            .map(|field| (field.name.as_str(), field.required))
            .collect();
        assert_eq!(accepted, accepted_oracle, "accepted members of {owner:?}");
        assert_eq!(
            registry.refused_members(owner).expect("a coherent ledger"),
            refused_oracle,
            "refused members of {owner:?}"
        );
    }
    assert_eq!(
        registry
            .refused_members(&record)
            .expect("a coherent ledger"),
        ["zz", "bb"],
        "refused members keep declaration order, not key order"
    );
}
