# docs/implementation/ — Agent Notes

This is the code-truth architecture map of Marrow: a thin, high-level guide that
says what each part does and where to read the real code. It mirrors the
pipeline (syntax to schema to check to runtime to store) so an agent can drill
from [README.md](README.md) down to one subsystem page.

Rules for this directory:
- It documents the ACTUAL code, not intended design. When code and a page
  disagree, the page is wrong — fix it.
- **You MUST update a page IN PLACE in the same change that makes a high-level
  change to the code it describes** — a module, type, pass, invariant, or data
  flow added, removed, renamed, or reshaped. Rewrite the stale lines and delete
  what no longer holds; make the smallest edit that makes the map true again.
- It is imperative these pages never accrue agentic sediment. Never append stale
  narration, duplicate lines, or changelog prose. A page is a thin map; if an
  edit makes it longer without making it truer, cut instead.
- Keep it a map, not a tutorial: reference files by repo-relative path and
  symbols by name, no line numbers, no slop.
- State a count once, in the table or list that enumerates it; never repeat the
  number in a heading or prose, where it drifts out of step with what it counts.
- Design and contract law lives elsewhere: `docs/language/` for the language,
  `docs/backend-contract.md` for the store contract. Link to them; do not
  restate them here.
