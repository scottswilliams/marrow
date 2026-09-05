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

/// Chains share a terminal spelling; no alias owns expanded syntax.
#[derive(Default)]
pub(super) struct AliasTable {
    terminals: Vec<Box<str>>,
    bindings: BTreeMap<String, AliasDenotation>,
}

#[derive(Clone, Copy)]
pub(crate) struct GlobalAliasTarget<'a> {
    pub(crate) name: &'a str,
    pub(crate) presence: AliasPresence,
}

pub(super) struct AliasInput<'a> {
    pub(super) at: FileRef,
    pub(super) file: &'a FileIdentity,
    pub(super) decl: &'a AliasDecl,
    pub(super) target: &'a str,
    pub(super) presence: AliasPresence,
}

impl AliasTable {
    pub(super) fn get(&self, name: &str) -> Option<GlobalAliasTarget<'_>> {
        let binding = self.bindings.get(name)?;
        Some(GlobalAliasTarget {
            name: &self.terminals[binding.terminal.0],
            presence: binding.presence,
        })
    }

    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    pub(super) fn remove(&mut self, name: &str) {
        self.bindings.remove(name);
    }

    pub(super) fn normalize(
        named: &mut DeclarationLedger<String, NamedTypeKind>,
        inputs: BTreeMap<String, AliasInput<'_>>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<Self, DeclareError> {
        let rows: Vec<_> = inputs.into_iter().collect();
        let index: BTreeMap<_, _> = rows
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.as_str(), index))
            .collect();
        let edges: Vec<_> = rows
            .iter()
            .map(|(_, input)| index.get(input.target).copied())
            .collect();
        let (order, cyclic) = dependency_order(&edges);
        for (node, (name, input)) in rows.iter().enumerate() {
            if !cyclic[node] {
                continue;
            }
            #[cfg(test)]
            bump_alias_cycle(|counts| counts.cyclic_aliases += 1);
            let refusal = refuse(
                diagnostics,
                DeclarationSite {
                    name,
                    file: input.file,
                    at: input.at,
                    span: input.decl.name_span,
                },
                Code::CheckRecursion.as_str(),
                format!("alias `{name}` is part of a cyclic alias chain"),
            );
            named.declare(name.clone(), DeclarationOccurrence::Refused(refusal))?;
        }
        let mut table = Self::default();
        let mut denotations: Vec<Option<AliasDenotation>> = vec![None; rows.len()];
        for node in order {
            if cyclic[node] {
                continue;
            }
            let (name, input) = &rows[node];
            let declared = DeclarationSite {
                name,
                file: input.file,
                at: input.at,
                span: input.decl.span,
            };
            let inherited = edges[node].and_then(|dependency| denotations[dependency]);
            let refusal = match named.lookup(input.target)? {
                Binding::Refused(_, summary) => {
                    Some(declaration_refused(input.file, input.decl.span, summary))
                }
                _ if inherited.is_some_and(|target| target.presence == AliasPresence::Optional)
                    && input.presence == AliasPresence::Optional =>
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
                named.declare(name.clone(), DeclarationOccurrence::Refused(refusal))?;
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
                    table.terminals.push(input.target.into());
                    #[cfg(test)]
                    bump_alias_cycle(|counts| {
                        counts.terminal_rows += 1;
                        counts.terminal_bytes += input.target.len();
                    });
                    AliasDenotation {
                        terminal,
                        presence: input.presence,
                    }
                }
            };
            denotations[node] = Some(target);
            table.bindings.insert(name.clone(), target);
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
            #[cfg(test)]
            bump_alias_cycle(|counts| {
                counts.node_entries += 1;
                counts.resolved_edges += usize::from(next.is_some());
                counts.edge_inspections += usize::from(next.is_some());
            });
        }
        while let Some(node) = stack.pop() {
            finished[node] = true;
            order.push(node);
        }
    }

    (order, cyclic)
}
