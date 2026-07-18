# Crate Contributor Notes

On the beta line this directory holds the retained core — the diagnostic-code
registry (`marrow-codes`), the syntax owner (`marrow-syntax`), the ordered-byte
storage engine (`marrow-store`), and the pure project-input owner
(`marrow-project`) — plus the crates refounded at T01: the storeless compiler
(`marrow-compile`), the image container owner (`marrow-image`), the independent
verifier (`marrow-verify`), the stack VM (`marrow-vm`), the path kernel
(`marrow-kernel`), and the `marrow` CLI. The prototype's
compiler, interpreter, catalog, and durable owners were deleted at B00 and are
being refounded lane by lane; a feature is absent until its lane lands it. The
nearest crate instructions apply in addition to the repository instructions.

Marrow is a general-purpose language designed to be built with production at
scale in mind: build each crate against what a widely used mainstream language
and its largest deployments require, never against what a prototype can get away
with. Current bounds are honest waypoints, not the bar. See the repository
`AGENTS.md` for the full statement.
