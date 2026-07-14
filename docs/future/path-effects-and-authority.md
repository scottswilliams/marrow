# Path effects and authority

This page is future direction. The current host checks and Bearer-authenticated
experimental serving do not implement compiler-integrated durable authority.

## Goal

The compiler describes each function's direct and transitive durable access
demand as typed operations over stable semantic regions. Demand is not
permission. Effective runtime access is the intersection of four independently
owned facts:

- verifier-reconstructed demand for the exact image;
- acceptance of the exact executable image and its changes;
- a separately owned maximum deployment ceiling; and
- an attenuated invocation grant.

One path kernel checks the instruction, binding, grant, operation, region,
contract, and transaction before it constructs a physical key or calls the
private engine. Every application durable instruction names a verifier-validated
effect site plus typed key operands. Demand describes need; it never grants the
access it discovers.

The compiler-described effects include compiler-derived index maintenance. The
beta's narrow compiler-maintained nonunique and unique indexes are maintained
atomically with their primary payload, and application code cannot write an
index; the maintenance an operation implies is part of the demand the verifier
reconstructs, not a separate application-issued write.

## Grants

A grant is export-scoped. It binds the exact image, export, and attachment
together with the export's reachable effect-site set and the stable demand atoms
that set implies, each intersected with the deployment ceiling. A grant carries
no atom the accepted image and export do not already demand.

A later externally authenticated principal can only further intersect a typed
address or context predicate at the kernel. It can narrow an already-granted
reach; it can never add an atom the grant did not carry.

## Host phase

A durable invocation has a host phase that ends and does not reopen. The host
phase closes at mutating-transaction entry or at the first read-only durable
opcode, whichever occurs first; the closed state (HostClosed) persists for the
rest of the invocation. Any host work an invocation performs occurs before that
point, so no host effect runs from the first durable access onward.

## Beta scope

The beta establishes structural containment for a local owner: exact bindings,
closed named exports, operation-specific effects over stable semantic regions,
visible conservative broadening, process-local unforgeable grants, and zero
engine calls for rejected images or invocations. Ordinary source does not repeat
inferred effect rows, authority clauses, ceilings, grants, store identities, or
proof witnesses.

The beta durable effect lattice uses a finite coarse reach for each operation
and stable semantic region: exact place, keyed layer, or subtree. Demand closes
over resolved direct calls by monotone fixed point. Keys derived from loaded
data widen visibly. Bounded operations carry compiler-known positive site
maxima; dynamic requested counts are checked at runtime rather than introducing
symbolic arithmetic into the effect lattice.

Beta authority is structural containment for a single local owner. It does not
provide users, passwords, roles, OAuth, application policy functions, public
clients, or a principal-level authorization product.

## Constraints

- Effect inference never grants the access it discovers.
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

## Deferred

The following remain later work and are not part of the beta floor:

- higher-order effect variables and closure capture of effects;
- recursive call-demand and indirect-call demand; the beta reconstructs demand
  over resolved direct calls only;
- fine argument-derived key provenance and public generic functions over places;
- authentication, principal and role systems, application policy language, and
  served enforcement.

## Evidence target

Compiler hover and update review must show direct and transitive operation,
region, coarse reach, traversal bound, and call-chain witnesses for Graph Report
and Club Locker. A harmless refactor must not silently grant access, and an
effect-broadening edit must remain inactive until independent deployment and
invocation authority covers it. Forged images, sites, paths, and grants must
reach zero engine calls.
