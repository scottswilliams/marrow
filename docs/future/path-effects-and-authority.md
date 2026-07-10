# Path effects and authority

This page is future direction. Current host checks and Bearer-authenticated
experimental serving do not implement compiler-integrated durable authority.

## Goal

The compiler should describe each function's direct and transitive durable
access demand as typed operations over semantic regions. That demand is not
permission. Effective access should require four independent facts:

- verifier-reconstructed demand for the exact image;
- acceptance of the exact candidate executable and its changes;
- a separately owned reusable maximum ceiling; and
- an invocation grant attenuated to one call, store, binding, transaction, and
  resource profile.

Every application durable instruction should name a verifier-validated effect
site plus typed key operands. One path kernel checks the instruction, binding,
grant, operation, region, contract, and transaction before constructing a
physical key or calling the private engine.

## Beta scope

The beta should establish structural containment for a local owner: exact
bindings, closed exported functions, operation-specific effects, visible
conservative broadening, process-local unforgeable grants, and zero engine calls
for rejected images or invocations.

It does not need users, passwords, roles, OAuth, application policy functions,
public clients, or a principal-level authorization product. Authentication and
served policy remain later work.

## Constraints

- Effect inference never grants the access it discovers.
- Higher-order functions and closures preserve effect upper bounds.
- Symbolic key provenance forms a finite algebra; recursive growth widens
  visibly rather than generating an unbounded term language.
- Imports grant nothing, and storeless host effects use a separate exact runner
  grant.
- Maintenance, activation, backup, restore, inspection, and physical recovery
  use authority types unavailable to application bytecode.
- Stored users, clients, credential verifiers, and rotation records are inert
  data, not authority. They cannot be decoded or restored into an authenticated
  context or invocation grant; any later authentication trust anchor remains
  outside application durable state.

Path enforcement alone does not prove noninterference, correct business policy,
confidential errors, regulatory compliance, or absence of timing channels.
