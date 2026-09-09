# Implementation Documentation Instructions

This directory maps the actual Marrow code. It helps contributors and tools
navigate the current source-to-image-to-runtime-to-store pipeline;
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
- Follow the [repository authority table](../../AGENTS.md#documentation-authority);
  link to the current reference instead of copying its semantics here.
- Plans, reports, and decision records have no normative authority here.
