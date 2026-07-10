# Implementation Documentation Instructions

This directory maps the actual Marrow code. It helps contributors and tools
navigate the current syntax-to-schema-to-check-to-runtime-to-store topology;
that topology is descriptive, not permanent architecture.

- Update a page in the same change that adds, removes, renames, or reshapes a
  high-level module, pass, invariant, or data flow.
- Rewrite stale lines in place. Do not append changelog narrative or preserve a
  prototype as historical context.
- Label legacy mechanisms plainly while they exist; do not normalize them as
  future design.
- Keep pages as maps to files and symbols, without line numbers or copied
  semantics.
- State counts once in the list that owns them.
- `docs/language/` owns current language behavior, `docs/vision.md` direction,
  `docs/design/` accepted unimplemented targets, `docs/status.md` implementation
  state, and `docs/backend-contract.md` the current storage contract.
- Plans, reports, and decision records have no normative authority here.
