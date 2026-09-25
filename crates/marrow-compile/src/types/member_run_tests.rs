//! Per-owner member reads against the whole member ledger.
//!
//! A record's field list and its refused members are read as one owner's contiguous
//! run of the ledger index. These tests declare owners whose keys would interleave
//! under any order that does not compare the whole owner first — a group anchor
//! `R.g`, names with `R` as a prefix, and the same spelling in a dependency tree —
//! and hold every owner's reads to its own members, first occurrence per name, in
//! declaration order.

use super::test_fixtures::test_registry;
use super::*;

use crate::decl::{DeclarationOccurrence, test_refusal};
use crate::diag::DiagnosticCollector;
use marrow_project::DependencyAlias;

enum Step {
    /// Accepted; the flag rides on `FieldInfo::required` so a second acceptance of
    /// one key is distinguishable from its first.
    Accept(bool),
    Refuse,
}

/// One owner, its accepted members as (name, required), and its refused members.
type OwnerReads<'a> = (&'a ScopedName, &'a [(&'a str, bool)], &'a [&'a str]);

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
            Refuse => DeclarationOccurrence::Refused(test_refusal(
                member,
                &mut DiagnosticCollector::new(),
            )),
        };
        registry
            .members
            .declare(MemberKey::new(owner, member), occurrence)
            .expect("within budget");
    }

    // `R` declares `title` twice; the first occurrence answers. Its refusals read in
    // declaration order, `zz` before `bb`.
    let expected: [OwnerReads; 5] = [
        (
            &record,
            &[("title", true), ("author", false), ("middle", true)],
            &["zz", "bb"],
        ),
        (&group, &[("year", false), ("edition", true)], &[]),
        (&prefixed, &[("pages", false), ("alpha", true)], &[]),
        (&dashed, &[("m", true), ("b", false)], &[]),
        (&foreign, &[("title", false), ("about", true)], &[]),
    ];
    for (owner, accepted, refused) in expected {
        let fields = registry.accepted_members(owner).expect("a coherent ledger");
        let fields: Vec<(&str, bool)> = fields
            .iter()
            .map(|field| (field.name.as_str(), field.required))
            .collect();
        assert_eq!(fields, accepted, "accepted members of {owner:?}");
        assert_eq!(
            registry.refused_members(owner).expect("a coherent ledger"),
            refused,
            "refused members of {owner:?}"
        );
    }
}
