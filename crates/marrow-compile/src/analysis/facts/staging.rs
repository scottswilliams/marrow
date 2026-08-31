//! Producer-bound custody for one lowered body.
//!
//! This child module is the privacy boundary: its wrapper never exposes a draft,
//! registry, diagnostic collector, staged facts owner, or generic callback. Each
//! operation below runs against the owners stored in the same aggregate, and every
//! release consumes that aggregate after its producer has committed or erased.

use marrow_codes::Code;
use marrow_image::{ExportId, FuncId, ImageDraft};
use marrow_project::FileIdentity;
use marrow_syntax::{Block, FunctionDecl, SourceSpan};

use super::{
    AnalysisFactCollector, DiagnosticCollector, FactSink, FileRef, ReleasedBody, StagedFacts,
};
use crate::compile::valid_export_path;
use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::ConstRegistry;
use crate::lower::{FnLowerer, FunctionRegistry, GenericRegistry, GenericTemplate, LowerResult};
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
        &'a mut self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        settled_facts: &'a AnalysisFactCollector,
        at: FileRef,
        file: &'a FileIdentity,
        module: &'a str,
        function: &'a FunctionDecl,
    ) -> LowerResult {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        let (registry, draft) = owner.parts();
        FnLowerer::lower(
            draft,
            registry,
            durable,
            functions,
            generics,
            consts,
            staged_diagnostics,
            staged_facts.sink(settled_facts, at),
            file,
            module,
            function,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_instance<'a>(
        &'a mut self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        template: &'a GenericTemplate<'a>,
        args: &[GArg],
    ) -> LowerResult {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts: _,
        } = self;
        let (registry, draft) = owner.parts();
        FnLowerer::lower_instance(
            draft,
            registry,
            durable,
            functions,
            generics,
            consts,
            staged_diagnostics,
            FactSink::discarding(),
            template,
            args,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_test<'a>(
        &'a mut self,
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
    ) -> LowerResult {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        let (registry, draft) = owner.parts();
        FnLowerer::lower_test(
            draft,
            registry,
            durable,
            functions,
            generics,
            consts,
            staged_diagnostics,
            staged_facts.sink(settled_facts, at),
            file,
            module,
            name,
            body,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_template<'a>(
        &'a mut self,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        settled_facts: &'a AnalysisFactCollector,
        template: &'a GenericTemplate<'a>,
    ) -> LowerResult {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        let (registry, draft) = owner.parts();
        FnLowerer::lower_template(
            draft,
            registry,
            durable,
            functions,
            generics,
            consts,
            staged_diagnostics,
            staged_facts.sink(settled_facts, template.at()),
            template,
        )
    }

    pub(crate) fn commit(self) -> ReleasedBody {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        owner.commit();
        Self::release(staged_diagnostics, staged_facts)
    }

    /// Validate and mint one public export inside the same custody boundary that owns
    /// the function body's diagnostics. An invalid path becomes this body's own row;
    /// neither branch exposes the staged collector or armed draft to the driver.
    pub(crate) fn commit_export(
        self,
        file: &FileIdentity,
        span: SourceSpan,
        module: &str,
        item: &str,
        func: FuncId,
    ) -> (ReleasedBody, Option<ExportId>) {
        let Self {
            mut owner,
            mut staged_diagnostics,
            staged_facts,
        } = self;
        let export = if valid_export_path(module, item) {
            let id = ExportId::of_local(module, item);
            owner.parts().1.add_export(id, func);
            Some(id)
        } else {
            staged_diagnostics.push(SourceDiagnostic::at(
                Code::CheckModulePath.as_str(),
                file,
                span,
                format!(
                    "export `{item}` in module `{module}` is not an ASCII identifier path, \
                     so it cannot be exported"
                ),
            ));
            None
        };
        owner.commit();
        (Self::release(staged_diagnostics, staged_facts), export)
    }

    /// Bind one lowered test title before committing the exact producer that emitted
    /// the function. A carrier-domain refusal abandons the aggregate as one unit.
    pub(crate) fn commit_test(
        self,
        name: &str,
        func: FuncId,
    ) -> Result<ReleasedBody, GenericInvariant> {
        let Self {
            mut owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        let draft = owner.parts().1;
        let name = draft
            .intern_string(name)
            .map_err(GenericInvariant::BuilderDomain)?;
        draft.add_test_entry(name, func);
        owner.commit();
        Ok(Self::release(staged_diagnostics, staged_facts))
    }

    /// Erase a template proof's throwaway producer, then release its generic-owner
    /// transfer and body payload together. Nothing can escape while the guard is armed.
    pub(crate) fn erase_proof(self) -> (GenericDiagnostics, ReleasedBody) {
        let Self {
            owner,
            staged_diagnostics,
            staged_facts,
        } = self;
        let generic = owner.registry().take_generic_diagnostics();
        owner.erase();
        (generic, Self::release(staged_diagnostics, staged_facts))
    }

    fn release(diagnostics: DiagnosticCollector, facts: StagedFacts) -> ReleasedBody {
        ReleasedBody {
            diagnostics: diagnostics.finish(),
            facts: facts.finish(),
        }
    }
}
