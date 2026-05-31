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
| P1 | Parser, formatter, CLI, diagnostics | 5 | Maybe | `feat-defaults`, `cli-doc-migration`, `lsp-check-diagnostics`, `lsp-retire-inrepo`, `enum-binding-index`, `enum-segment-precision` | `algo-collatz#3`, `algo-sieve-primes#1`, `app-library-catalog#6`, `app-versioned-cms#6`, `cluster-modules-params#3` |
| P5 | Identity and nominal type consistency | 12 | No | `identity-key-static-reject`, `fix-resource-ctor-runtime`, `element-loop-semantics`, `unkeyed-required-fields` | `algo-set-ops-keyedtree#1`, `app-banking-locks#5`, `app-registrar-composite-id#2`, `app-url-shortener#5`, `app-task-tracker#2`, `app-dependency-graph#8`, `app-inventory-warehouse#1`, `app-versioned-cms#0`, `app-audit-log#3`, `cluster-resources-identity#0`, `cluster-resources-identity#2`, `fuzz-11#1` |
| P6 | Conversions, literals, temporal boundaries | 5 | No | `literal-escape-decode`, `enum-segment-precision` | `cluster-clock-duration#2`, `cluster-conversions-unknown#3`, `cluster-conversions-unknown#4`, `cluster-enums#1`, `cluster-numerics-decimal#4` |
| P8 | Type surfaces for reads and traversal | 8 | No | `element-loop-semantics`, `lsp-check-diagnostics` | `algo-ackermann#0`, `algo-fizzbuzz#3`, `app-calendar-scheduler#0`, `app-double-entry-ledger#0`, `app-dependency-graph#0`, `app-double-entry-ledger#3`, `cluster-indexes#2`, `cluster-indexes#4` |
| P9 | Local collections | 9 | No | `element-loop-semantics`, `feat-defaults` | `algo-collatz#2`, `algo-date-daycount-leap#5`, `algo-insertion-sort#1`, `algo-palindrome-utf8#1`, `algo-roman-numerals#4`, `app-calendar-scheduler#4`, `apps:app-ttl-cache#1`, `app-dependency-graph#4`, `app-audit-log#5` |
| P10 | Saved storage, indexes, presence | 3 | No | `unkeyed-required-fields`, `saved-walk-cursor`, `element-loop-semantics` | `app-calendar-scheduler#1`, `cluster-indexes#1`, `fuzz-9#1` |
| P11 | Traversal, neighbors, mutation guards | 10 | No | `element-loop-semantics`, `saved-walk-cursor` | `algo-csv-splitter#1`, `algo-csv-splitter#2`, `app-registrar-composite-id#1`, `app-library-catalog#7`, `cluster-controlflow-errors#3`, `cluster-sequences-traversal#0`, `cluster-sequences-traversal#1`, `cluster-sequences-traversal#2`, `cluster-sequences-traversal#3`, `fuzz-9#0` |

Open issue count: 52.
