//! Producer-bound custody for one lowered body.
//!
//! This child module is the privacy boundary: its wrapper never exposes a draft,
//! registry, diagnostic collector, staged facts owner, or generic callback. Each
//! operation below runs against the owners stored in the same aggregate, and every
//! release consumes that aggregate after its producer has committed or erased.

use crate::source::ProjectFile;
use marrow_codes::Code;
use marrow_image::{ExportId, ImageDraft};
use marrow_syntax::{Block, FunctionDecl};

use super::{
    AnalysisFactCollector, DiagnosticCollector, FactSink, FileRef, ReleasedBody, StagedFacts,
};
use crate::compile::valid_export_path;
use crate::diag::SourceDiagnostic;
use crate::lower::{BodyOutcome, FnLowerer, GenericTemplate, LowerCtx, Resolution};
use crate::types::{GArg, GenericDiagnostics, GenericInvariant, GenericOwnerTxn, TypeRegistry};

/// Where one declared body was written: the retained coordinate, the owned identity a
/// diagnostic renders, and the dotted module its export path is built from.
#[derive(Clone, Copy)]
pub(crate) struct BodySite<'a> {
    pub(crate) at: FileRef,
    pub(crate) file: &'a ProjectFile,
    pub(crate) module: &'a str,
}

/// One generic-owner producer and both payloads it may publish.
///
/// The fields are private even to the parent module. A whole value may move, but no safe
/// caller can detach or exchange any one of its four owners while a producer is armed.
pub(crate) struct StagedBodyTxn<'r, 'd> {
    owner: GenericOwnerTxn<'r, 'd>,
    staged_diagnostics: DiagnosticCollector,
    staged_facts: StagedFacts,
}

impl<'r, 'd> StagedBodyTxn<'r, 'd> {
    pub(crate) fn begin(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        Ok(Self::new(GenericOwnerTxn::begin(registry, draft)?))
    }

    pub(crate) fn enter_proof(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        Ok(Self::new(GenericOwnerTxn::enter_proof(registry, draft)?))
    }

    fn new(owner: GenericOwnerTxn<'r, 'd>) -> Self {
        Self {
            owner,
            staged_diagnostics: DiagnosticCollector::new(),
            staged_facts: StagedFacts::new(),
        }
    }

    pub(crate) fn lower_function<'a>(
        self,
        resolution: Resolution<'a, 'a>,
        settled_facts: &'a AnalysisFactCollector,
        site: BodySite<'a>,
        function: &'a FunctionDecl,
        func: marrow_image::FuncId,
    ) -> Result<(ReleasedBody, BodyOutcome, Option<ExportId>), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        let BodySite { at, file, module } = site;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower(
                LowerCtx {
                    draft,
                    records: registry,
                    resolution,
                    diagnostics: &mut staged_diagnostics,
                    facts: staged_facts.sink(settled_facts, at),
                },
                file,
                module,
                function,
                func,
            )?
        };
        let export = match &outcome {
            BodyOutcome::Lowered(lowered) if function.public => {
                if valid_export_path(module, &function.name) {
                    let id = ExportId::of_local(module, &function.name);
                    owner.parts().1.add_export(id, lowered.func);
                    Some(id)
                } else {
                    staged_diagnostics.push(SourceDiagnostic::at(
                        Code::CheckModulePath,
                        file,
                        function.span,
                        format!(
                            "export `{}` in module `{module}` is not an ASCII identifier path, \
                             so it cannot be exported",
                            function.name
                        ),
                    ));
                    None
                }
            }
            BodyOutcome::Lowered(_) | BodyOutcome::Refused => None,
        };
        owner.commit();
        Ok((
            Self::release(staged_diagnostics, staged_facts),
            outcome,
            export,
        ))
    }

    pub(crate) fn lower_instance<'a>(
        self,
        resolution: Resolution<'a, 'a>,
        template: &'a GenericTemplate<'a>,
        args: &[GArg],
        func: marrow_image::FuncId,
    ) -> Result<(ReleasedBody, BodyOutcome), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            staged_facts,
        } = self;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower_instance(
                LowerCtx {
                    draft,
                    records: registry,
                    resolution,
                    diagnostics: &mut staged_diagnostics,
                    facts: FactSink::discarding(),
                },
                template,
                args,
                func,
            )?
        };
        owner.commit();
        Ok((Self::release(staged_diagnostics, staged_facts), outcome))
    }

    pub(crate) fn lower_test<'a>(
        self,
        resolution: Resolution<'a, 'a>,
        settled_facts: &'a AnalysisFactCollector,
        site: BodySite<'a>,
        name: &'a str,
        body: &'a Block,
        func: marrow_image::FuncId,
    ) -> Result<(ReleasedBody, BodyOutcome), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        let BodySite { at, file, module } = site;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower_test(
                LowerCtx {
                    draft,
                    records: registry,
                    resolution,
                    diagnostics: &mut staged_diagnostics,
                    facts: staged_facts.sink(settled_facts, at),
                },
                file,
                module,
                name,
                body,
                func,
            )?
        };
        if let BodyOutcome::Lowered(lowered) = &outcome {
            let draft = owner.parts().1;
            let name = draft
                .intern_string(name)
                .map_err(GenericInvariant::BuilderDomain)?;
            draft.add_test_entry(name, lowered.func);
        }
        owner.commit();
        Ok((Self::release(staged_diagnostics, staged_facts), outcome))
    }

    pub(crate) fn prove_template<'a>(
        self,
        resolution: Resolution<'a, 'a>,
        settled_facts: &'a AnalysisFactCollector,
        template: &'a GenericTemplate<'a>,
    ) -> Result<(GenericDiagnostics, ReleasedBody), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        {
            let (registry, draft) = owner.parts();
            FnLowerer::lower_template(
                LowerCtx {
                    draft,
                    records: registry,
                    resolution,
                    diagnostics: &mut staged_diagnostics,
                    facts: staged_facts.sink(settled_facts, template.at()),
                },
                template,
            )?;
        }
        let generic = owner.registry().take_generic_diagnostics();
        owner.erase();
        Ok((generic, Self::release(staged_diagnostics, staged_facts)))
    }

    fn release(diagnostics: DiagnosticCollector, facts: StagedFacts) -> ReleasedBody {
        ReleasedBody {
            diagnostics: diagnostics.finish(),
            facts: facts.finish(),
        }
    }
}
