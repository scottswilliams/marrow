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
| P1 | Parser, formatter, CLI, diagnostics | 22 | Maybe | `feat-defaults`, `cli-doc-migration`, `lsp-check-diagnostics`, `lsp-retire-inrepo`, `enum-binding-index`, `enum-segment-precision` | `algo-collatz#3`, `algo-json-tokenizer#0`, `algo-matrix-multiply#1`, `algo-merge-sort#0`, `algo-merge-sort#1`, `algo-run-length-encode#1`, `algo-sieve-primes#1`, `app-expression-interpreter#5`, `app-double-entry-ledger#1`, `app-library-catalog#6`, `app-versioned-cms#6`, `cluster-cli-config-fmt#0`, `cluster-cli-config-fmt#1`, `cluster-cli-config-fmt#3`, `cluster-cli-config-fmt#4`, `cluster-controlflow-errors#0`, `cluster-controlflow-errors#1`, `cluster-controlflow-errors#2`, `cluster-modules-params#3`, `app-versioned-cms#4`, `fuzz-6#0`, `fuzz-6#1` |
| P4 | Resource constructors and local resource values | 10 | No | `fix-resource-ctor-runtime`, `identity-key-static-reject` | `algo-compound-interest-decimal#3`, `algo-matrix-multiply#0`, `app-expression-interpreter#0`, `app-mini-spreadsheet#3`, `app-inventory-warehouse#2`, `app-library-catalog#2`, `app-dependency-graph#3`, `cluster-resources-identity#1`, `fuzz-10#0`, `fuzz-2#0` |
| P5 | Identity and nominal type consistency | 12 | No | `identity-key-static-reject`, `fix-resource-ctor-runtime`, `element-loop-semantics`, `unkeyed-required-fields` | `algo-set-ops-keyedtree#1`, `app-banking-locks#5`, `app-registrar-composite-id#2`, `app-url-shortener#5`, `app-task-tracker#2`, `app-dependency-graph#8`, `app-inventory-warehouse#1`, `app-versioned-cms#0`, `app-audit-log#3`, `cluster-resources-identity#0`, `cluster-resources-identity#2`, `fuzz-11#1` |
| P6 | Conversions, literals, temporal boundaries | 22 | No | `literal-escape-decode`, `enum-segment-precision` | `algo-base64-roundtrip#2`, `algo-compound-interest-decimal#1`, `algo-compound-interest-decimal#2`, `algo-csv-splitter#0`, `algo-date-daycount-leap#3`, `algo-json-tokenizer#3`, `app-banking-locks#1`, `app-fsm-engine#0`, `app-url-shortener#1`, `app-library-catalog#0`, `cluster-clock-duration#2`, `cluster-conversions-unknown#0`, `cluster-conversions-unknown#1`, `cluster-conversions-unknown#3`, `cluster-conversions-unknown#4`, `cluster-enums#1`, `cluster-numerics-decimal#1`, `cluster-numerics-decimal#4`, `cluster-strings-bytes#0`, `cluster-strings-bytes#1`, `cluster-strings-bytes#2`, `fuzz-11#0` |
| P8 | Type surfaces for reads and traversal | 8 | No | `element-loop-semantics`, `lsp-check-diagnostics` | `algo-ackermann#0`, `algo-fizzbuzz#3`, `app-calendar-scheduler#0`, `app-double-entry-ledger#0`, `app-dependency-graph#0`, `app-double-entry-ledger#3`, `cluster-indexes#2`, `cluster-indexes#4` |
| P9 | Local collections | 11 | No | `element-loop-semantics`, `feat-defaults` | `algo-collatz#1`, `algo-collatz#2`, `algo-date-daycount-leap#5`, `algo-insertion-sort#1`, `algo-palindrome-utf8#1`, `algo-roman-numerals#4`, `app-calendar-scheduler#4`, `apps:app-ttl-cache#1`, `app-dependency-graph#4`, `app-dependency-graph#5`, `app-audit-log#5` |
| P10 | Saved storage, indexes, presence | 14 | No | `unkeyed-required-fields`, `saved-walk-cursor`, `element-loop-semantics` | `app-calendar-scheduler#1`, `app-mini-spreadsheet#0`, `app-url-shortener#3`, `app-dependency-graph#1`, `cluster-backup-restore#0`, `cluster-clock-duration#3`, `cluster-indexes#1`, `cluster-indexes#3`, `cluster-saved-encoding-integrity#1`, `cluster-sparse-presence#0`, `cluster-sparse-presence#1`, `cluster-sparse-presence#2`, `cluster-sparse-presence#3`, `fuzz-9#1` |
| P11 | Traversal, neighbors, mutation guards | 10 | No | `element-loop-semantics`, `saved-walk-cursor` | `algo-csv-splitter#1`, `algo-csv-splitter#2`, `app-registrar-composite-id#1`, `app-library-catalog#7`, `cluster-controlflow-errors#3`, `cluster-sequences-traversal#0`, `cluster-sequences-traversal#1`, `cluster-sequences-traversal#2`, `cluster-sequences-traversal#3`, `fuzz-9#0` |
| P12 | Enums | 4 | Maybe | `enum-binding-index`, `enum-segment-precision` | `app-expression-interpreter#2`, `app-expression-interpreter#4`, `cluster-enums#2`, `cluster-enums#3` |

Open issue count: 113.
