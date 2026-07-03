//! The single owner of diagnostic prose for codes built through
//! [`CheckDiagnostic::new`](crate::CheckDiagnostic::new). A diagnostic's human
//! message is a pure function of its registry [`Code`] and typed
//! [`DiagnosticPayload`]: the construction site supplies typed facts, and the
//! message is rendered here, once, so prose is never built beside the facts.

use marrow_codes::Code;

use crate::diagnostics::{
    AmbiguousMemberForm, DefaultEntryProblem, DiagnosticPayload, EnumDiagnostic, IsTypeFault,
    MatchScrutinee, SurfaceActionDiagnostic, SurfaceComputedReadDiagnostic, SurfaceFieldDiagnostic,
    SurfaceFieldProblem, SurfaceRootOrigin, SurfaceTargetDiagnostic,
};
use crate::typerules::{marrow_type_name, mismatch_display};

/// The codes whose prose is owned by [`render_message`]. Their construction sites
/// pass a typed payload to [`CheckDiagnostic::new`](crate::CheckDiagnostic::new) and
/// build no message, so a message-bearing `CheckDiagnostic::error`/`warning` call
/// must never name one. The `no_prose_at_migrated_construction` scan enforces that;
/// extend this list as each diagnostic family migrates.
pub(crate) const MIGRATED_CODES: &[Code] = &[
    Code::CheckReturnType,
    Code::CheckAssignmentType,
    Code::CheckDefaultEntry,
    Code::CheckMultipleScripts,
    Code::CheckSurfaceTarget,
    Code::CheckSurfaceField,
    Code::CheckSurfaceAction,
    Code::CheckSurfaceComputedRead,
    Code::CheckUnknownEnumMember,
    Code::CheckAmbiguousMember,
    Code::CheckAmbiguousMatchArm,
    Code::CheckScrutineeQualifiedMatchArm,
    Code::CheckNonexhaustiveMatch,
    Code::CheckDuplicateMatchArm,
    Code::CheckMatchRequiresEnum,
    Code::CheckIsRequiresEnum,
    Code::CheckIsType,
];

/// Render the human message for a migrated `(code, payload)` pair. Total over
/// [`MIGRATED_CODES`] with their emitted payloads, which is every pair
/// [`CheckDiagnostic::new`](crate::CheckDiagnostic::new) can reach.
pub(crate) fn render_message(code: Code, payload: &DiagnosticPayload) -> String {
    debug_assert!(
        MIGRATED_CODES.contains(&code),
        "render_message reached for {code:?}, which CheckDiagnostic::new does not own yet",
    );
    match (code, payload) {
        (Code::CheckReturnType, DiagnosticPayload::TypeMismatch { expected, found }) => {
            let (expected, found) = mismatch_display(expected, found);
            format!("function returns `{expected}`, but this value is `{found}`")
        }
        (Code::CheckAssignmentType, DiagnosticPayload::TypeMismatch { expected, found }) => {
            let (expected, found) = mismatch_display(expected, found);
            format!("expected `{expected}`, but the value is `{found}`")
        }
        (Code::CheckDefaultEntry, DiagnosticPayload::DefaultEntry { entry, problem }) => {
            format!(
                "`run.defaultEntry` `{entry}` {}",
                default_entry_reason(*problem)
            )
        }
        (Code::CheckMultipleScripts, DiagnosticPayload::None) => "a project may have at most \
             one file without a `module` declaration (its single-file script); declare a \
             `module` for this file"
            .to_string(),
        (Code::CheckSurfaceTarget, DiagnosticPayload::SurfaceTarget(target)) => {
            surface_target_message(target)
        }
        (Code::CheckSurfaceField, DiagnosticPayload::SurfaceField(field)) => {
            surface_field_message(field)
        }
        (Code::CheckSurfaceAction, DiagnosticPayload::SurfaceAction(action)) => {
            surface_action_message(action)
        }
        (Code::CheckSurfaceComputedRead, DiagnosticPayload::SurfaceComputedRead(read)) => {
            surface_computed_read_message(read)
        }
        (
            Code::CheckUnknownEnumMember
            | Code::CheckAmbiguousMember
            | Code::CheckAmbiguousMatchArm
            | Code::CheckScrutineeQualifiedMatchArm
            | Code::CheckNonexhaustiveMatch
            | Code::CheckDuplicateMatchArm
            | Code::CheckMatchRequiresEnum
            | Code::CheckIsRequiresEnum
            | Code::CheckIsType,
            DiagnosticPayload::Enum(diagnostic),
        ) => enum_message(diagnostic),
        (code, payload) => {
            unreachable!("no message template for {code:?} with payload {payload:?}")
        }
    }
}

fn enum_message(diagnostic: &EnumDiagnostic) -> String {
    match diagnostic {
        EnumDiagnostic::UnknownMember {
            enum_name,
            member,
            suggestions,
        } => {
            let mut message = format!("`{enum_name}` has no member `{member}`");
            if let [only] = suggestions.as_slice() {
                message.push_str(&format!("; did you mean `{only}`?"));
            } else if !suggestions.is_empty() {
                message.push_str(&format!("; did you mean {}?", join_or(suggestions)));
            }
            message
        }
        EnumDiagnostic::AmbiguousMember {
            enum_name,
            label,
            candidates,
            form,
        } => {
            let path = format!("{enum_name}::{label}");
            match form {
                AmbiguousMemberForm::BareForeignOwner => {
                    format!("`{path}` is ambiguous; qualify as {}", join_or(candidates))
                }
                AmbiguousMemberForm::ValuePosition => format!(
                    "`{path}` names more than one member of `{enum_name}`; qualify as {}",
                    join_or(candidates)
                ),
            }
        }
        EnumDiagnostic::AmbiguousMatchArm {
            enum_name,
            label,
            candidates,
        } => format!(
            "`{label}` names more than one member of `{enum_name}`; qualify as {}",
            join_or(candidates)
        ),
        EnumDiagnostic::ScrutineeQualifiedMatchArm {
            enum_name,
            written,
            relative,
        } => format!(
            "`match` arms are relative to the scrutinee enum `{enum_name}`; \
             write the arm as `{relative}`, not `{written}`"
        ),
        EnumDiagnostic::NonexhaustiveMatch { enum_name, missing } => format!(
            "`match` on `{enum_name}` does not cover {}",
            missing
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        EnumDiagnostic::DuplicateMatchArm { label } => {
            format!("`match` has a duplicate arm for `{label}`")
        }
        EnumDiagnostic::MatchRequiresEnum(scrutinee) => match scrutinee {
            MatchScrutinee::UndeclaredEnum { enum_name } => format!(
                "`match` requires an enum value, but the scrutinee's enum `{enum_name}` is not declared"
            ),
            MatchScrutinee::NonEnum { found } => format!(
                "`match` requires an enum value, but the scrutinee is `{}`",
                marrow_type_name(found)
            ),
        },
        EnumDiagnostic::IsRequiresEnum { found } => format!(
            "operator `is` requires an enum value on the left, but found `{}`",
            marrow_type_name(found)
        ),
        EnumDiagnostic::IsType(fault) => match fault {
            IsTypeFault::RequiresMember { left_name } => {
                format!("operator `is` requires a member of `{left_name}` on the right")
            }
            IsTypeFault::DifferentEnum {
                left_name,
                right_name,
            } => format!(
                "operator `is` compares within one enum, but the left is `{left_name}` and the right names `{right_name}`"
            ),
        },
        EnumDiagnostic::CategoryNotSelectable { .. } => {
            unreachable!("CategoryNotSelectable is not built through CheckDiagnostic::new")
        }
    }
}

/// Join member paths into an actionable "qualify as `a` or `b`" hint, each path
/// quoted. One path drops the "or"; the join is total over any candidate list.
fn join_or(paths: &[String]) -> String {
    let quoted: Vec<String> = paths.iter().map(|path| format!("`{path}`")).collect();
    match quoted.as_slice() {
        [one] => one.clone(),
        [head @ .., last] => format!("{} or {last}", head.join(", ")),
        [] => String::new(),
    }
}

fn surface_target_message(target: &SurfaceTargetDiagnostic) -> String {
    match target {
        SurfaceTargetDiagnostic::UnknownStore { origin, root } => match origin {
            SurfaceRootOrigin::Surface { name } => {
                format!("surface `{name}` targets unknown store `^{root}`")
            }
            SurfaceRootOrigin::Collection => {
                format!("surface collection targets unknown store `^{root}`")
            }
        },
        SurfaceTargetDiagnostic::AmbiguousStore { origin, root } => match origin {
            SurfaceRootOrigin::Surface { name } => {
                format!("surface `{name}` targets ambiguous store root `^{root}`")
            }
            SurfaceRootOrigin::Collection => {
                format!("surface collection targets ambiguous store root `^{root}`")
            }
        },
        SurfaceTargetDiagnostic::InvalidStore { surface, root } => {
            format!("surface `{surface}` targets invalid backing store `^{root}`")
        }
        SurfaceTargetDiagnostic::InvalidStoreResource {
            surface,
            root,
            resource,
        } => {
            format!(
                "surface `{surface}` targets store `^{root}` with invalid resource `{resource}`"
            )
        }
        SurfaceTargetDiagnostic::AmbiguousStoreResource {
            surface,
            root,
            resource,
        } => {
            format!(
                "surface `{surface}` targets store `^{root}` with ambiguous resource `{resource}`"
            )
        }
        SurfaceTargetDiagnostic::ForeignCollectionRoot {
            surface_root,
            target_root,
        } => {
            format!(
                "surface collection target `^{target_root}` is not backing store `^{surface_root}`"
            )
        }
        SurfaceTargetDiagnostic::KeylessCollectionRoot { root } => {
            format!("surface collection targets keyless singleton root `^{root}`")
        }
        SurfaceTargetDiagnostic::UnknownCollectionIndex { root, index } => {
            format!("surface collection names no index `{index}` on `^{root}`")
        }
        SurfaceTargetDiagnostic::AmbiguousCollectionIndex { root, index } => {
            format!("surface collection names ambiguous index `{index}` on `^{root}`")
        }
        SurfaceTargetDiagnostic::InvalidCollectionIndex { root, index } => {
            format!("surface collection names schema-invalid index `{index}` on `^{root}`")
        }
        SurfaceTargetDiagnostic::RangeCollectionUniqueIndex { root, index } => {
            format!("surface range collection targets unique index `{index}` on `^{root}`")
        }
        SurfaceTargetDiagnostic::RangeCollectionMissingIdentitySuffix { root, index } => {
            format!(
                "surface range collection index `{index}` on `^{root}` does not end with the store identity"
            )
        }
        SurfaceTargetDiagnostic::RangeCollectionMissingRangeKey { root, index } => {
            format!(
                "surface range collection index `{index}` on `^{root}` has no non-identity range key"
            )
        }
        SurfaceTargetDiagnostic::RangeCollectionUnsupportedRangeKey { root, index, key } => {
            format!(
                "surface range collection index `{index}` on `^{root}` ranges over non-scalar key `{key}`"
            )
        }
    }
}

fn surface_field_message(field: &SurfaceFieldDiagnostic) -> String {
    let SurfaceFieldDiagnostic {
        list,
        name,
        problem,
    } = field;
    let list = list.label();
    match problem {
        SurfaceFieldProblem::Unknown => {
            format!("surface {list} item `{name}` is not a top-level backing field")
        }
        SurfaceFieldProblem::Unsupported => {
            format!("surface {list} item `{name}` is not a plain top-level field")
        }
        SurfaceFieldProblem::Invalid => {
            format!("surface {list} item `{name}` names a schema-invalid backing field")
        }
        SurfaceFieldProblem::Ambiguous => {
            format!("surface {list} item `{name}` names an ambiguous backing field")
        }
        SurfaceFieldProblem::NotProjected => {
            format!("surface {list} item `{name}` must also appear in `fields`")
        }
        SurfaceFieldProblem::RequiredNotCreateAddressable => {
            format!("surface create item must include required backing field `{name}`")
        }
        SurfaceFieldProblem::IdentityKey => {
            format!(
                "surface {list} item `{name}` names an identity key; identity keys are returned \
                 automatically under `identity` in every read and page response, so they cannot \
                 be listed in `fields`"
            )
        }
    }
}

fn surface_action_message(action: &SurfaceActionDiagnostic) -> String {
    match action {
        SurfaceActionDiagnostic::UnknownFunction { path } => {
            format!("surface action targets unknown function `{path}`")
        }
        SurfaceActionDiagnostic::PrivateFunction { path } => {
            format!("surface action targets private function `{path}`")
        }
        SurfaceActionDiagnostic::AmbiguousFunction { path } => {
            format!("surface action targets ambiguous function `{path}`")
        }
        SurfaceActionDiagnostic::UnsupportedParameter { path, parameter } => {
            format!(
                "surface action `{path}` parameter `{parameter}` has a type outside the action JSON surface"
            )
        }
        SurfaceActionDiagnostic::UnsupportedReturn { path } => {
            format!("surface action `{path}` return type is outside the action JSON surface")
        }
    }
}

fn surface_computed_read_message(read: &SurfaceComputedReadDiagnostic) -> String {
    match read {
        SurfaceComputedReadDiagnostic::UnknownFunction { path } => {
            format!("surface computed read targets unknown function `{path}`")
        }
        SurfaceComputedReadDiagnostic::PrivateFunction { path } => {
            format!("surface computed read targets private function `{path}`")
        }
        SurfaceComputedReadDiagnostic::AmbiguousFunction { path } => {
            format!("surface computed read targets ambiguous function `{path}`")
        }
        SurfaceComputedReadDiagnostic::UnsupportedParameter { path, parameter } => {
            format!(
                "surface computed read `{path}` parameter `{parameter}` has a type outside the computed-read JSON surface"
            )
        }
        SurfaceComputedReadDiagnostic::UnsupportedReturn { path } => {
            format!(
                "surface computed read `{path}` return type is outside the computed-read JSON surface"
            )
        }
        SurfaceComputedReadDiagnostic::Writes { path } => {
            format!("surface computed read `{path}` may write saved data")
        }
        SurfaceComputedReadDiagnostic::Transactions { path } => {
            format!("surface computed read `{path}` may open a transaction")
        }
        SurfaceComputedReadDiagnostic::HostEffects { path } => {
            format!("surface computed read `{path}` may call host effects")
        }
        SurfaceComputedReadDiagnostic::Throws { path } => {
            format!("surface computed read `{path}` may throw")
        }
        SurfaceComputedReadDiagnostic::UnindexedCollectionRead { path } => {
            format!("surface computed read `{path}` may read an unindexed collection")
        }
    }
}

fn default_entry_reason(problem: DefaultEntryProblem) -> &'static str {
    match problem {
        DefaultEntryProblem::Missing => "names no public entry",
        DefaultEntryProblem::Private => "names a private function; mark it `pub`",
        DefaultEntryProblem::Ambiguous => "is ambiguous; qualify it as `module::function`",
        DefaultEntryProblem::HasParameters => {
            "declares parameters, but a default entry runs with no arguments"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MIGRATED_CODES, render_message};
    use crate::diagnostics::{
        AmbiguousMemberForm, DefaultEntryProblem, DiagnosticPayload, EnumDiagnostic, IsTypeFault,
        MatchScrutinee, SurfaceActionDiagnostic, SurfaceComputedReadDiagnostic,
        SurfaceFieldDiagnostic, SurfaceFieldList, SurfaceFieldProblem, SurfaceRootOrigin,
        SurfaceTargetDiagnostic,
    };
    use crate::program::MarrowType;
    use marrow_codes::Code;
    use marrow_store::value::ScalarType;
    use std::path::{Path, PathBuf};

    fn primitive(scalar: ScalarType) -> MarrowType {
        MarrowType::Primitive(scalar)
    }

    /// Every migrated `(code, payload)` renders exactly the message its old
    /// construction site built. This pins the prose the renderer now owns, so a
    /// drift from the original wording fails here.
    #[test]
    fn renders_migrated_prose_byte_identical() {
        assert_eq!(
            render_message(
                Code::CheckReturnType,
                &DiagnosticPayload::TypeMismatch {
                    expected: primitive(ScalarType::Int),
                    found: primitive(ScalarType::Str),
                },
            ),
            "function returns `int`, but this value is `string`",
        );
        assert_eq!(
            render_message(
                Code::CheckAssignmentType,
                &DiagnosticPayload::TypeMismatch {
                    expected: primitive(ScalarType::Bool),
                    found: primitive(ScalarType::Int),
                },
            ),
            "expected `bool`, but the value is `int`",
        );
        for (problem, reason) in [
            (DefaultEntryProblem::Missing, "names no public entry"),
            (
                DefaultEntryProblem::Private,
                "names a private function; mark it `pub`",
            ),
            (
                DefaultEntryProblem::Ambiguous,
                "is ambiguous; qualify it as `module::function`",
            ),
            (
                DefaultEntryProblem::HasParameters,
                "declares parameters, but a default entry runs with no arguments",
            ),
        ] {
            assert_eq!(
                render_message(
                    Code::CheckDefaultEntry,
                    &DiagnosticPayload::DefaultEntry {
                        entry: "main".to_string(),
                        problem,
                    },
                ),
                format!("`run.defaultEntry` `main` {reason}"),
            );
        }
        assert_eq!(
            render_message(Code::CheckMultipleScripts, &DiagnosticPayload::None),
            "a project may have at most one file without a `module` declaration \
             (its single-file script); declare a `module` for this file",
        );
    }

    /// The surface diagnostic families render exactly the message their old
    /// construction sites built, across every payload variant they emit. Pins the
    /// prose the renderer now owns for the surface codes.
    #[test]
    fn renders_surface_prose_byte_identical() {
        let target = |t| {
            render_message(
                Code::CheckSurfaceTarget,
                &DiagnosticPayload::SurfaceTarget(t),
            )
        };
        assert_eq!(
            target(SurfaceTargetDiagnostic::UnknownStore {
                origin: SurfaceRootOrigin::Surface {
                    name: "Books".into()
                },
                root: "books".into(),
            }),
            "surface `Books` targets unknown store `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::UnknownStore {
                origin: SurfaceRootOrigin::Collection,
                root: "books".into(),
            }),
            "surface collection targets unknown store `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::AmbiguousStore {
                origin: SurfaceRootOrigin::Surface {
                    name: "Books".into()
                },
                root: "books".into(),
            }),
            "surface `Books` targets ambiguous store root `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::AmbiguousStore {
                origin: SurfaceRootOrigin::Collection,
                root: "books".into(),
            }),
            "surface collection targets ambiguous store root `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::InvalidStore {
                surface: "Books".into(),
                root: "books".into(),
            }),
            "surface `Books` targets invalid backing store `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::InvalidStoreResource {
                surface: "Books".into(),
                root: "books".into(),
                resource: "Book".into(),
            }),
            "surface `Books` targets store `^books` with invalid resource `Book`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::AmbiguousStoreResource {
                surface: "Books".into(),
                root: "books".into(),
                resource: "Book".into(),
            }),
            "surface `Books` targets store `^books` with ambiguous resource `Book`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::ForeignCollectionRoot {
                surface_root: "books".into(),
                target_root: "authors".into(),
            }),
            "surface collection target `^authors` is not backing store `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::KeylessCollectionRoot {
                root: "config".into()
            }),
            "surface collection targets keyless singleton root `^config`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::UnknownCollectionIndex {
                root: "books".into(),
                index: "byMissing".into(),
            }),
            "surface collection names no index `byMissing` on `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::AmbiguousCollectionIndex {
                root: "books".into(),
                index: "byTitle".into(),
            }),
            "surface collection names ambiguous index `byTitle` on `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::InvalidCollectionIndex {
                root: "books".into(),
                index: "byTitle".into(),
            }),
            "surface collection names schema-invalid index `byTitle` on `^books`",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::RangeCollectionUniqueIndex {
                root: "books".into(),
                index: "byTitle".into(),
            }),
            "surface range collection targets unique index `byTitle` on `^books`",
        );
        assert_eq!(
            target(
                SurfaceTargetDiagnostic::RangeCollectionMissingIdentitySuffix {
                    root: "books".into(),
                    index: "byTitle".into(),
                }
            ),
            "surface range collection index `byTitle` on `^books` does not end with the store identity",
        );
        assert_eq!(
            target(SurfaceTargetDiagnostic::RangeCollectionMissingRangeKey {
                root: "books".into(),
                index: "byTitle".into(),
            }),
            "surface range collection index `byTitle` on `^books` has no non-identity range key",
        );
        assert_eq!(
            target(
                SurfaceTargetDiagnostic::RangeCollectionUnsupportedRangeKey {
                    root: "books".into(),
                    index: "byTitle".into(),
                    key: "title".into(),
                }
            ),
            "surface range collection index `byTitle` on `^books` ranges over non-scalar key `title`",
        );

        let field = |problem, list| {
            render_message(
                Code::CheckSurfaceField,
                &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                    list,
                    name: "meta".into(),
                    problem,
                }),
            )
        };
        assert_eq!(
            field(SurfaceFieldProblem::Unknown, SurfaceFieldList::Fields),
            "surface fields item `meta` is not a top-level backing field",
        );
        assert_eq!(
            field(SurfaceFieldProblem::Unsupported, SurfaceFieldList::Fields),
            "surface fields item `meta` is not a plain top-level field",
        );
        assert_eq!(
            field(SurfaceFieldProblem::Invalid, SurfaceFieldList::Update),
            "surface update item `meta` names a schema-invalid backing field",
        );
        assert_eq!(
            field(SurfaceFieldProblem::Ambiguous, SurfaceFieldList::Fields),
            "surface fields item `meta` names an ambiguous backing field",
        );
        assert_eq!(
            field(SurfaceFieldProblem::NotProjected, SurfaceFieldList::Create),
            "surface create item `meta` must also appear in `fields`",
        );
        assert_eq!(
            field(
                SurfaceFieldProblem::RequiredNotCreateAddressable,
                SurfaceFieldList::Create
            ),
            "surface create item must include required backing field `meta`",
        );
        assert_eq!(
            field(SurfaceFieldProblem::IdentityKey, SurfaceFieldList::Fields),
            "surface fields item `meta` names an identity key; identity keys are returned \
             automatically under `identity` in every read and page response, so they cannot \
             be listed in `fields`",
        );

        let action = |a| {
            render_message(
                Code::CheckSurfaceAction,
                &DiagnosticPayload::SurfaceAction(a),
            )
        };
        assert_eq!(
            action(SurfaceActionDiagnostic::UnknownFunction {
                path: "app::save".into()
            }),
            "surface action targets unknown function `app::save`",
        );
        assert_eq!(
            action(SurfaceActionDiagnostic::PrivateFunction {
                path: "app::save".into()
            }),
            "surface action targets private function `app::save`",
        );
        assert_eq!(
            action(SurfaceActionDiagnostic::AmbiguousFunction {
                path: "save".into()
            }),
            "surface action targets ambiguous function `save`",
        );
        assert_eq!(
            action(SurfaceActionDiagnostic::UnsupportedParameter {
                path: "app::save".into(),
                parameter: "raw".into(),
            }),
            "surface action `app::save` parameter `raw` has a type outside the action JSON surface",
        );
        assert_eq!(
            action(SurfaceActionDiagnostic::UnsupportedReturn {
                path: "app::save".into()
            }),
            "surface action `app::save` return type is outside the action JSON surface",
        );

        let read = |r| {
            render_message(
                Code::CheckSurfaceComputedRead,
                &DiagnosticPayload::SurfaceComputedRead(r),
            )
        };
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::UnknownFunction {
                path: "app::view".into()
            }),
            "surface computed read targets unknown function `app::view`",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::PrivateFunction {
                path: "app::view".into()
            }),
            "surface computed read targets private function `app::view`",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::AmbiguousFunction {
                path: "view".into()
            }),
            "surface computed read targets ambiguous function `view`",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::UnsupportedParameter {
                path: "app::view".into(),
                parameter: "raw".into(),
            }),
            "surface computed read `app::view` parameter `raw` has a type outside the computed-read JSON surface",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::UnsupportedReturn {
                path: "app::view".into()
            }),
            "surface computed read `app::view` return type is outside the computed-read JSON surface",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::Writes {
                path: "app::view".into()
            }),
            "surface computed read `app::view` may write saved data",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::Transactions {
                path: "app::view".into()
            }),
            "surface computed read `app::view` may open a transaction",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::HostEffects {
                path: "app::view".into()
            }),
            "surface computed read `app::view` may call host effects",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::Throws {
                path: "app::view".into()
            }),
            "surface computed read `app::view` may throw",
        );
        assert_eq!(
            read(SurfaceComputedReadDiagnostic::UnindexedCollectionRead {
                path: "app::view".into()
            }),
            "surface computed read `app::view` may read an unindexed collection",
        );
    }

    /// The enum diagnostic family renders exactly the message its old construction
    /// sites built, across every migrated variant. Both `AmbiguousMember` forms are
    /// exercised, since the two share their facts and differ only by the stored
    /// discriminant. Pins the prose the renderer now owns for the enum codes.
    #[test]
    fn renders_enum_prose_byte_identical() {
        let enum_msg =
            |code, diagnostic| render_message(code, &DiagnosticPayload::Enum(diagnostic));

        assert_eq!(
            enum_msg(
                Code::CheckUnknownEnumMember,
                EnumDiagnostic::UnknownMember {
                    enum_name: "Animal".into(),
                    member: "tabby".into(),
                    suggestions: vec![],
                },
            ),
            "`Animal` has no member `tabby`",
        );
        assert_eq!(
            enum_msg(
                Code::CheckUnknownEnumMember,
                EnumDiagnostic::UnknownMember {
                    enum_name: "Animal".into(),
                    member: "tabby".into(),
                    suggestions: vec!["Animal::mammal::cat::tabby".into()],
                },
            ),
            "`Animal` has no member `tabby`; did you mean `Animal::mammal::cat::tabby`?",
        );
        assert_eq!(
            enum_msg(
                Code::CheckUnknownEnumMember,
                EnumDiagnostic::UnknownMember {
                    enum_name: "Animal".into(),
                    member: "tabby".into(),
                    suggestions: vec!["Animal::cat::tabby".into(), "Animal::tabby".into()],
                },
            ),
            "`Animal` has no member `tabby`; did you mean `Animal::cat::tabby` or `Animal::tabby`?",
        );

        assert_eq!(
            enum_msg(
                Code::CheckAmbiguousMember,
                EnumDiagnostic::AmbiguousMember {
                    enum_name: "Cat".into(),
                    label: "paw".into(),
                    candidates: vec!["Cat::tiger::paw".into(), "Cat::lion::paw".into()],
                    form: AmbiguousMemberForm::BareForeignOwner,
                },
            ),
            "`Cat::paw` is ambiguous; qualify as `Cat::tiger::paw` or `Cat::lion::paw`",
        );
        assert_eq!(
            enum_msg(
                Code::CheckAmbiguousMember,
                EnumDiagnostic::AmbiguousMember {
                    enum_name: "Cat".into(),
                    label: "paw".into(),
                    candidates: vec!["Cat::tiger::paw".into(), "Cat::lion::paw".into()],
                    form: AmbiguousMemberForm::ValuePosition,
                },
            ),
            "`Cat::paw` names more than one member of `Cat`; qualify as `Cat::tiger::paw` or `Cat::lion::paw`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckAmbiguousMatchArm,
                EnumDiagnostic::AmbiguousMatchArm {
                    enum_name: "Cat".into(),
                    label: "paw".into(),
                    candidates: vec!["tiger::paw".into(), "lion::paw".into()],
                },
            ),
            "`paw` names more than one member of `Cat`; qualify as `tiger::paw` or `lion::paw`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckScrutineeQualifiedMatchArm,
                EnumDiagnostic::ScrutineeQualifiedMatchArm {
                    enum_name: "Status".into(),
                    written: "Status::active".into(),
                    relative: "active".into(),
                },
            ),
            "`match` arms are relative to the scrutinee enum `Status`; \
             write the arm as `active`, not `Status::active`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckNonexhaustiveMatch,
                EnumDiagnostic::NonexhaustiveMatch {
                    enum_name: "Status".into(),
                    missing: vec!["active".into(), "closed".into()],
                },
            ),
            "`match` on `Status` does not cover `active`, `closed`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckDuplicateMatchArm,
                EnumDiagnostic::DuplicateMatchArm {
                    label: "active".into(),
                },
            ),
            "`match` has a duplicate arm for `active`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckMatchRequiresEnum,
                EnumDiagnostic::MatchRequiresEnum(MatchScrutinee::UndeclaredEnum {
                    enum_name: "Status".into(),
                }),
            ),
            "`match` requires an enum value, but the scrutinee's enum `Status` is not declared",
        );
        assert_eq!(
            enum_msg(
                Code::CheckMatchRequiresEnum,
                EnumDiagnostic::MatchRequiresEnum(MatchScrutinee::NonEnum {
                    found: primitive(ScalarType::Int),
                }),
            ),
            "`match` requires an enum value, but the scrutinee is `int`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckIsRequiresEnum,
                EnumDiagnostic::IsRequiresEnum {
                    found: primitive(ScalarType::Bool),
                },
            ),
            "operator `is` requires an enum value on the left, but found `bool`",
        );

        assert_eq!(
            enum_msg(
                Code::CheckIsType,
                EnumDiagnostic::IsType(IsTypeFault::RequiresMember {
                    left_name: "Status".into(),
                }),
            ),
            "operator `is` requires a member of `Status` on the right",
        );
        assert_eq!(
            enum_msg(
                Code::CheckIsType,
                EnumDiagnostic::IsType(IsTypeFault::DifferentEnum {
                    left_name: "Status".into(),
                    right_name: "Color".into(),
                }),
            ),
            "operator `is` compares within one enum, but the left is `Status` and the right names `Color`",
        );
    }

    /// The identifiers a migrated code would appear as in a first argument to a
    /// message-bearing `CheckDiagnostic::error`/`warning` call: its `Code` variant
    /// and its `CHECK_*` wire-string constant. Mirrors [`MIGRATED_CODES`]; kept in
    /// step by the length assertion in `no_prose_at_migrated_construction`.
    const MIGRATED_CONSTRUCTION_TOKENS: &[&str] = &[
        "Code::CheckReturnType",
        "CHECK_RETURN_TYPE",
        "Code::CheckAssignmentType",
        "CHECK_ASSIGNMENT_TYPE",
        "Code::CheckDefaultEntry",
        "CHECK_DEFAULT_ENTRY",
        "Code::CheckMultipleScripts",
        "CHECK_MULTIPLE_SCRIPTS",
        "Code::CheckSurfaceTarget",
        "CHECK_SURFACE_TARGET",
        "Code::CheckSurfaceField",
        "CHECK_SURFACE_FIELD",
        "Code::CheckSurfaceAction",
        "CHECK_SURFACE_ACTION",
        "Code::CheckSurfaceComputedRead",
        "CHECK_SURFACE_COMPUTED_READ",
        "Code::CheckUnknownEnumMember",
        "CHECK_UNKNOWN_ENUM_MEMBER",
        "Code::CheckAmbiguousMember",
        "CHECK_AMBIGUOUS_MEMBER",
        "Code::CheckAmbiguousMatchArm",
        "CHECK_AMBIGUOUS_MATCH_ARM",
        "Code::CheckScrutineeQualifiedMatchArm",
        "CHECK_SCRUTINEE_QUALIFIED_MATCH_ARM",
        "Code::CheckNonexhaustiveMatch",
        "CHECK_NONEXHAUSTIVE_MATCH",
        "Code::CheckDuplicateMatchArm",
        "CHECK_DUPLICATE_MATCH_ARM",
        "Code::CheckMatchRequiresEnum",
        "CHECK_MATCH_REQUIRES_ENUM",
        "Code::CheckIsRequiresEnum",
        "CHECK_IS_REQUIRES_ENUM",
        "Code::CheckIsType",
        "CHECK_IS_TYPE",
    ];

    fn src_root() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
    }

    fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("src entry").path();
            if path.is_dir() {
                rust_sources(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }

    /// The first argument of a call, given the text just after its opening `(`:
    /// everything up to the first top-level comma or the closing `)`.
    fn first_argument(after_open_paren: &str) -> &str {
        let mut depth = 0usize;
        for (index, byte) in after_open_paren.bytes().enumerate() {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 => return &after_open_paren[..index],
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => return &after_open_paren[..index],
                _ => {}
            }
        }
        after_open_paren
    }

    /// The message-bearing constructors take a `message` argument, so their code is
    /// the first argument. A migrated code must never appear there: its prose lives
    /// only in `render_message`, reached through the message-less
    /// `CheckDiagnostic::new`.
    ///
    /// Blind spots, as with the L3/L4 tidy scans: this matches only the literal
    /// `CheckDiagnostic::error(`/`warning(` spellings and the hand-maintained tokens
    /// above, so an aliased or renamed constructor, or a code assembled at runtime,
    /// would slip past. Reviewers block those the same way.
    #[test]
    fn no_prose_at_migrated_construction() {
        assert_eq!(
            MIGRATED_CONSTRUCTION_TOKENS.len(),
            MIGRATED_CODES.len() * 2,
            "MIGRATED_CONSTRUCTION_TOKENS must list the Code variant and CHECK_* constant \
             for every code in MIGRATED_CODES",
        );
        let mut files = Vec::new();
        rust_sources(&src_root(), &mut files);
        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("read rust source");
            for constructor in ["CheckDiagnostic::error(", "CheckDiagnostic::warning("] {
                for (index, _) in text.match_indices(constructor) {
                    let after = &text[index + constructor.len()..];
                    let code_arg = first_argument(after);
                    for token in MIGRATED_CONSTRUCTION_TOKENS {
                        if code_arg.contains(token) {
                            offenders.push(format!("{}: {constructor}{token}", file.display()));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "migrated codes must be built through CheckDiagnostic::new, not a message-bearing \
             constructor:\n{}",
            offenders.join("\n"),
        );
    }
}
