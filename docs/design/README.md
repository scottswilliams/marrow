# Target Design Contracts

This directory contains no accepted target contracts after the documentation
reset.

A target contract answers one narrow question: which exact unimplemented rule
has been approved for implementation? It does not describe current behavior;
the [Language Reference](../language/) and implementation continue to do that.
It does not replace the high-level [Vision](../vision.md).

Approval comes from the human project maintainer responsible for Marrow's
language and product direction, or from a human maintainer explicitly delegated
that authority. An agent, lane owner, reviewer, test, or prototype cannot accept
its own proposal.

## Lifecycle

1. **Research.** Alternatives and unanswered questions are explored in a
   short-lived review artifact. Research has no design authority.
2. **Proposal.** A complete target contract is reviewed against current
   behavior, the vision, and affected trust boundaries. A proposal remains
   non-authoritative.
3. **Accepted target.** After explicit human project-owner approval, the
   contract is added here and named by the orchestration master plan. It governs
   only the work needed to implement that target.
4. **Current behavior.** The implementation and canonical current references
   change together. The target contract is then deleted rather than retained as
   a second specification.
5. **Rationale.** A decision record may preserve why the choice was made after
   the canonical documentation owns the result.

Git history retains research and replaced target contracts. The active
documentation does not retain drafts or completed designs for context.

## Required Contents

An accepted target contract names:

- the human owner and explicit acceptance event;
- its scope and the current behavior it replaces;
- normative rules, invariants, and failure behavior;
- trust assumptions and explicitly deferred questions outside its scope;
- no unresolved normative question within its accepted scope;
- affected reference and implementation owners;
- conformance evidence required before integration; and
- the condition under which the contract moves into current references and is
  deleted.

“Accepted target” is not evidence of implementation. [Project Status](../status.md)
must continue to classify the behavior as unimplemented until a reachable
supported non-test implementation and its required evidence exist.
