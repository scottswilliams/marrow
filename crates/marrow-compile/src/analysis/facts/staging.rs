//! Producer-bound custody for one lowered body.
//!
//! This child module is the privacy boundary: its wrapper never exposes a draft,
//! registry, diagnostic collector, staged facts owner, or generic callback. Each
//! operation below runs against the owners stored in the same aggregate, and every
//! release consumes that aggregate after its producer has committed or erased.

use crate::source::ProjectFile;
use marrow_image::{FuncId, ImageDraft};
use marrow_syntax::{FunctionDecl, TestDecl};

use super::{
    AnalysisFactCollector, DiagnosticCollector, FactSink, FileRef, ReleasedBody, StagedFacts,
};
use crate::lower::{BodyOutcome, BodyRole, FnLowerer, GenericTemplate, LowerCtx, Resolution};
use crate::types::{GArg, GenericDiagnostics, GenericInvariant, GenericOwnerTxn, TypeRegistry};

/// Where one declared body was written: the retained coordinate, the owned identity a
/// diagnostic renders, and the dotted module its unqualified calls resolve in.
#[derive(Clone, Copy)]
pub(crate) struct BodySite<'a> {
    pub(crate) at: FileRef,
    pub(crate) file: &'a ProjectFile,
    pub(crate) module: &'a str,
}

/// One body to lower under an armed producer: the lowerer entry it takes and where its
/// editor facts go. A generic instance's facts were collected once at its template's
/// proof, so it stages none.
#[derive(Clone, Copy)]
pub(crate) enum BodyToLower<'a> {
    Function {
        site: BodySite<'a>,
        function: &'a FunctionDecl,
        func: FuncId,
        role: BodyRole,
    },
    Test {
        site: BodySite<'a>,
        test: &'a TestDecl,
        func: FuncId,
    },
    Instance {
        template: &'a GenericTemplate<'a>,
        args: &'a [GArg],
        func: FuncId,
    },
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

    /// Lower one body under this armed producer and commit: the body's interns, site
    /// requests, function fill and every registry row its mints appended land as one
    /// unit, and an ordinary refusal commits too, since the rows it minted can be
    /// referenced from outside it.
    pub(crate) fn lower<'a>(
        self,
        resolution: Resolution<'a, 'a>,
        settled_facts: &'a AnalysisFactCollector,
        body: BodyToLower<'a>,
    ) -> Result<(ReleasedBody, BodyOutcome), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
            mut staged_facts,
        } = self;
        let outcome = {
            let (registry, draft) = owner.parts();
            let facts = match body {
                BodyToLower::Function { site, .. } | BodyToLower::Test { site, .. } => {
                    staged_facts.sink(settled_facts, site.at)
                }
                BodyToLower::Instance { .. } => FactSink::discarding(),
            };
            let ctx = LowerCtx {
                draft,
                records: registry,
                resolution,
                diagnostics: &mut staged_diagnostics,
                facts,
            };
            match body {
                BodyToLower::Function {
                    site,
                    function,
                    func,
                    role,
                } => FnLowerer::lower(ctx, site.file, site.module, function, func, role)?,
                BodyToLower::Test { site, test, func } => {
                    FnLowerer::lower_test(ctx, site.file, site.module, test, func)?
                }
                BodyToLower::Instance {
                    template,
                    args,
                    func,
                } => FnLowerer::lower_instance(ctx, template, args, func)?,
            }
        };
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
