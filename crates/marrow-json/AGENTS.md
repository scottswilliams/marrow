# marrow-json Contributor Notes

This crate owns bounded JSON representations of compiler, runtime, and tooling
facts. JSON is a boundary encoding; it does not own language types, semantic
paths, routes, authorization, or execution.

Wire mirrors convert exhaustively from their upstream typed owners so a new
variant cannot drift silently. Serialization must not touch or materialize
durable data omitted from the requested result. Prose belongs in renderers;
consumers branch on typed codes and fields.

Resource-schema DTOs are current compiler-derived facts consumed by tooling;
keep them exhaustive and independent of transport semantics. Surface-request
DTOs, route manifests, and serving helpers describe the legacy application
boundary. Do not extend or stabilize that model. New boundary work must consume
compiler-owned semantic path and authority facts after its target contract is
accepted.

Map: [docs/implementation/json.md](../../docs/implementation/json.md).
