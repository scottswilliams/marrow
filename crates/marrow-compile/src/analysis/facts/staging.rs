//! Producer-bound custody for one lowered body.
//!
//! This child module is the privacy boundary: its wrapper never exposes a draft,
//! registry, diagnostic collector, staged facts owner, or generic callback. Each
//! operation below runs against the owners stored in the same aggregate, and every
//! release consumes that aggregate after its producer has committed or erased.

use marrow_codes::Code;
use marrow_image::{ExportId, ImageDraft};
use marrow_project::FileIdentity;
use marrow_syntax::{Block, FunctionDecl};

use super::{
    AnalysisFactCollector, DiagnosticCollector, FactSink, FileRef, ReleasedBody, StagedFacts,
};
use crate::compile::valid_export_path;
use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::ConstRegistry;
use crate::lower::{BodyOutcome, FnLowerer, FunctionRegistry, GenericRegistry, GenericTemplate};
use crate::types::{GArg, GenericDiagnostics, GenericInvariant, GenericOwnerTxn, TypeRegistry};

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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_function<'a>(
        self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        settled_facts: &'a AnalysisFactCollector,
        at: FileRef,
        file: &'a FileIdentity,
        module: &'a str,
        function: &'a FunctionDecl,
    ) -> Result<(ReleasedBody, BodyOutcome, Option<ExportId>), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower(
                draft,
                registry,
                durable,
                functions,
                generics,
                consts,
                &mut staged_diagnostics,
                staged_facts.sink(settled_facts, at),
                file,
                module,
                function,
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
                        Code::CheckModulePath.as_str(),
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_instance<'a>(
        self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        template: &'a GenericTemplate<'a>,
        args: &[GArg],
    ) -> Result<(ReleasedBody, BodyOutcome), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            staged_facts,
        } = self;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower_instance(
                draft,
                registry,
                durable,
                functions,
                generics,
                consts,
                &mut staged_diagnostics,
                FactSink::discarding(),
                template,
                args,
            )?
        };
        owner.commit();
        Ok((Self::release(staged_diagnostics, staged_facts), outcome))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_test<'a>(
        self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        settled_facts: &'a AnalysisFactCollector,
        at: FileRef,
        file: &'a FileIdentity,
        module: &'a str,
        name: &'a str,
        body: &'a Block,
    ) -> Result<(ReleasedBody, BodyOutcome), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        let outcome = {
            let (registry, draft) = owner.parts();
            FnLowerer::lower_test(
                draft,
                registry,
                durable,
                functions,
                generics,
                consts,
                &mut staged_diagnostics,
                staged_facts.sink(settled_facts, at),
                file,
                module,
                name,
                body,
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prove_template<'a>(
        self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
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
                draft,
                registry,
                durable,
                functions,
                generics,
                consts,
                &mut staged_diagnostics,
                staged_facts.sink(settled_facts, template.at()),
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
