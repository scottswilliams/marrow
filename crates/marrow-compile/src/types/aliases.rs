//! Type aliases: each supported alias is stored as a shared global terminal name
//! and an optionality. Chains normalize iteratively and unsupported target shapes
//! refuse before dependent fills, so type consumers resolve written parameters
//! before aliases and never allocate an expanded alias tree.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AliasPresence {
    Bare,
    Optional,
}

#[derive(Clone, Copy)]
struct AliasTerminalId(usize);

#[derive(Clone, Copy)]
struct AliasDenotation {
    terminal: AliasTerminalId,
    presence: AliasPresence,
}

/// Chains share a terminal name; no alias owns expanded syntax. A terminal is
/// scoped to the tree whose alias chain bound it, so a dependency's alias never
/// terminates in the consumer's namespace.
#[derive(Default)]
pub(super) struct AliasTable {
    terminals: Vec<ScopedName>,
    bindings: BTreeMap<ScopedName, AliasDenotation>,
}

#[derive(Clone, Copy)]
pub(crate) struct GlobalAliasTarget<'a> {
    pub(crate) terminal: &'a ScopedName,
    pub(crate) presence: AliasPresence,
}

pub(super) struct AliasInput<'a> {
    pub(super) at: FileRef,
    pub(super) file: &'a ProjectFile,
    pub(super) decl: &'a AliasDecl,
    /// Where the written target resolves, or `None` when the spelling names no
    /// tree — a qualified target whose first segment is not a declared dependency.
    pub(super) target: Option<ScopedName>,
    pub(super) presence: AliasPresence,
}

impl AliasTable {
    pub(super) fn get(&self, scope: &ScopedName) -> Option<GlobalAliasTarget<'_>> {
        let binding = self.bindings.get(scope)?;
        Some(GlobalAliasTarget {
            terminal: &self.terminals[binding.terminal.0],
            presence: binding.presence,
        })
    }

    pub(super) fn contains_key(&self, scope: &ScopedName) -> bool {
        self.bindings.contains_key(scope)
    }

    pub(super) fn remove(&mut self, scope: &ScopedName) {
        self.bindings.remove(scope);
    }

    pub(super) fn normalize(
        named: &mut DeclarationLedger<ScopedName, NamedTypeKind>,
        inputs: BTreeMap<ScopedName, AliasInput<'_>>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<Self, DeclareError> {
        let rows: Vec<_> = inputs.into_iter().collect();
        let index: BTreeMap<_, _> = rows
            .iter()
            .enumerate()
            .map(|(index, (scope, _))| (scope.clone(), index))
            .collect();
        let edges: Vec<_> = rows
            .iter()
            .map(|(_, input)| {
                input
                    .target
                    .as_ref()
                    .and_then(|target| index.get(target).copied())
            })
            .collect();
        let (order, cyclic) = dependency_order(&edges);
        for (node, (scope, input)) in rows.iter().enumerate() {
            if !cyclic[node] {
                continue;
            }
            let name = scope.name();
            let refusal = refuse(
                diagnostics,
                DeclarationSite {
                    name,
                    file: input.file,
                    at: input.at,
                    span: input.decl.name_span,
                },
                Code::CheckRecursion,
                format!("alias `{name}` is part of a cyclic alias chain"),
            );
            named.declare(scope.clone(), DeclarationOccurrence::Refused(refusal))?;
        }
        let mut table = Self::default();
        let mut denotations: Vec<Option<AliasDenotation>> = vec![None; rows.len()];
        for node in order {
            if cyclic[node] {
                continue;
            }
            let (scope, input) = &rows[node];
            let name = scope.name();
            let declared = DeclarationSite {
                name,
                file: input.file,
                at: input.at,
                span: input.decl.span,
            };
            let inherited = edges[node].and_then(|dependency| denotations[dependency]);
            // A target that names no tree is outside the admitted set, for the same
            // reason an unknown bare name is: nothing declares it.
            let target_binding = match &input.target {
                Some(target) => named.lookup(target)?,
                None => Binding::Absent,
            };
            let refusal = match target_binding {
                Binding::Refused(_, summary) => {
                    Some(declaration_refused(input.file, input.decl.span, summary))
                }
                _ if input.target.is_none()
                    || (inherited
                        .is_some_and(|target| target.presence == AliasPresence::Optional)
                        && input.presence == AliasPresence::Optional) =>
                {
                    Some(unsupported(
                        input.file,
                        input.decl.span,
                        &format!("the target type of alias `{name}`"),
                    ))
                }
                _ => None,
            };
            if let Some(row) = refusal {
                let refusal = refuse_row(diagnostics, declared, row);
                named.declare(scope.clone(), DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            let target = match inherited {
                Some(mut target) => {
                    if input.presence == AliasPresence::Optional {
                        target.presence = AliasPresence::Optional;
                    }
                    target
                }
                None => {
                    let terminal = AliasTerminalId(table.terminals.len());
                    // The refusal arm above returned for every `None` target.
                    let Some(written) = input.target.clone() else {
                        continue;
                    };
                    table.terminals.push(written);
                    AliasDenotation {
                        terminal,
                        presence: input.presence,
                    }
                }
            };
            denotations[node] = Some(target);
            table.bindings.insert(scope.clone(), target);
        }
        Ok(table)
    }
}

/// Each candidate has at most one dependency. An explicit path classifies every
/// cycle and produces dependency-first completion without revisiting chains.
fn dependency_order(edges: &[Option<usize>]) -> (Vec<usize>, Vec<bool>) {
    let mut visited = vec![false; edges.len()];
    let mut finished = vec![false; edges.len()];
    let mut cyclic = vec![false; edges.len()];
    let mut order = Vec::with_capacity(edges.len());
    let mut stack = Vec::new();
    for root in 0..edges.len() {
        let mut next = Some(root);
        while let Some(node) = next {
            if finished[node] {
                break;
            }
            if visited[node] {
                // Every active node belongs to this one explicit path.
                for &member in stack.iter().rev() {
                    cyclic[member] = true;
                    if member == node {
                        break;
                    }
                }
                break;
            }
            visited[node] = true;
            stack.push(node);
            next = edges[node];
        }
        while let Some(node) = stack.pop() {
            finished[node] = true;
            order.push(node);
        }
    }

    (order, cyclic)
}
