# Agent Instructions

This repository is Marrow.

Marrow is a lightweight, typed `.mw` language with built-in saved data. A
resource is a typed tree; the same shape can be local or saved, and `^` marks
saved data.

`docs/language/` is the canonical source for Marrow language behavior. Parser,
checker, runtime, CLI, LSP, examples, tests, and other docs converge on that
directory. When implementation and documentation disagree, treat the
disagreement as implementation work, not as a competing design.

## Product Rules

- Build `.mw` as its own language and database model.
- Do not add alternate language modes to the default product.
- Do not bundle database-specific adapters in the first release.
- Keep native storage as the normal local project store.
- Keep saved data inspectable through Marrow tools.
- Prefer deleting stale scaffolding over preserving confusing transitional
  layers.

## Engineering Rules

- Keep edits small, direct, and reviewable.
- Match the reference docs before inventing new behavior.
- Add tests near behavior as soon as implementation exists.
- Use simple Rust and narrow abstractions.
- Do not introduce `unsafe` Rust.
- Do not keep durable agent notes, transcripts, bulky logs, or speculative
  design drafts in the repository.

## Repository Shape

- `docs/language/` is the language reference.
- `docs/implementation.md` is the implementation and backend reference.
- `docs/roadmap/` tracks implementation order.
- Future source, tests, examples, and editor integrations should be added only
  when they match the reference docs.

## Verification

Documentation-only work uses `git diff --check` and stale-link/name scans.
Implementation work starts with focused tests and grows to workspace checks as
the codebase grows.
