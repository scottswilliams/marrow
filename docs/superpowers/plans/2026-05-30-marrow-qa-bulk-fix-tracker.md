# Marrow QA Bulk Fix Tracker

This tracker contains only open work. When a package is fully fixed, verified,
reviewed, and integrated, delete its row. If a package is partly fixed, split
the row and leave only unresolved IDs here.

Source ledgers:

- `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery/findings/verified.jsonl`
- `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery/findings/followups.jsonl`

## Open Work Packages

| Package | Area | Count | Design Input | Conflict Watch | Finding IDs |
|---|---|---:|---|---|---|
| P5 | Identity and nominal type consistency | 5 | No | `identity-key-static-reject`, `element-loop-semantics`, `unkeyed-required-fields` | `algo-set-ops-keyedtree#1`, `app-registrar-composite-id#2`, `app-task-tracker#2`, `app-dependency-graph#8`, `app-audit-log#3` |
| P8 | Type surfaces for reads and traversal | 2 | No | `element-loop-semantics`, `lsp-check-diagnostics` | `algo-fizzbuzz#3`, `app-double-entry-ledger#3` |
| P11 | Traversal, neighbors, mutation guards | 2 | No | `element-loop-semantics`, `saved-walk-cursor` | `algo-csv-splitter#1`, `algo-csv-splitter#2` |

Open issue count: 9.
