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
| P0 | Design decisions | 6 | Yes | dirty `docs/language/control-flow-and-effects.md`; `literal-escape-decode`; `cli-doc-migration` | `algo-csv-splitter#3`, `algo-gcd-euclid#0`, `algo-palindrome-utf8#2`, `cluster-clock-duration#5`, `cluster-sparse-presence#5`, `followup-presence-require#0` |
| P1 | Parser, formatter, CLI, diagnostics | 25 | Maybe | `feat-defaults`, `cli-doc-migration`, `lsp-check-diagnostics`, `lsp-retire-inrepo`, `enum-binding-index`, `enum-segment-precision` | `algo-collatz#3`, `algo-json-tokenizer#0`, `algo-matrix-multiply#1`, `algo-merge-sort#0`, `algo-merge-sort#1`, `algo-run-length-encode#1`, `algo-sieve-primes#1`, `app-expression-interpreter#5`, `app-double-entry-ledger#1`, `app-library-catalog#6`, `app-versioned-cms#6`, `cluster-cli-config-fmt#0`, `cluster-cli-config-fmt#1`, `cluster-cli-config-fmt#2`, `cluster-cli-config-fmt#3`, `cluster-cli-config-fmt#4`, `cluster-cli-config-fmt#5`, `cluster-controlflow-errors#0`, `cluster-controlflow-errors#1`, `cluster-controlflow-errors#2`, `cluster-modules-params#3`, `algo-factorial#4`, `app-versioned-cms#4`, `fuzz-6#0`, `fuzz-6#1` |
| P2 | Modules, parameters, call contracts | 6 | No | `lsp-check-diagnostics` if diagnostics change | `algo-roman-numerals#3`, `apps:app-ttl-cache#4`, `cluster-modules-params#0`, `cluster-modules-params#1`, `cluster-modules-params#2`, `cluster-modules-params#6` |
| P3 | Module constants | 5 | No | none known | `algo-json-tokenizer#2`, `algo-palindrome-utf8#0`, `app-expression-interpreter#3`, `app-mini-spreadsheet#2`, `app-url-shortener#0` |
| P4 | Resource constructors and local resource values | 10 | No | `fix-resource-ctor-runtime`, `identity-key-static-reject` | `algo-compound-interest-decimal#3`, `algo-matrix-multiply#0`, `app-expression-interpreter#0`, `app-mini-spreadsheet#3`, `app-inventory-warehouse#2`, `app-library-catalog#2`, `app-dependency-graph#3`, `cluster-resources-identity#1`, `fuzz-10#0`, `fuzz-2#0` |
| P5 | Identity and nominal type consistency | 12 | No | `identity-key-static-reject`, `fix-resource-ctor-runtime`, `element-loop-semantics`, `unkeyed-required-fields` | `algo-set-ops-keyedtree#1`, `app-banking-locks#5`, `app-registrar-composite-id#2`, `app-url-shortener#5`, `app-task-tracker#2`, `app-dependency-graph#8`, `app-inventory-warehouse#1`, `app-versioned-cms#0`, `app-audit-log#3`, `cluster-resources-identity#0`, `cluster-resources-identity#2`, `fuzz-11#1` |
| P6 | Conversions, literals, temporal boundaries | 22 | Yes | `literal-escape-decode`, `enum-segment-precision` | `algo-base64-roundtrip#2`, `algo-compound-interest-decimal#1`, `algo-compound-interest-decimal#2`, `algo-csv-splitter#0`, `algo-date-daycount-leap#3`, `algo-json-tokenizer#3`, `app-banking-locks#1`, `app-fsm-engine#0`, `app-url-shortener#1`, `app-library-catalog#0`, `cluster-clock-duration#2`, `cluster-conversions-unknown#0`, `cluster-conversions-unknown#1`, `cluster-conversions-unknown#3`, `cluster-conversions-unknown#4`, `cluster-enums#1`, `cluster-numerics-decimal#1`, `cluster-numerics-decimal#4`, `cluster-strings-bytes#0`, `cluster-strings-bytes#1`, `cluster-strings-bytes#2`, `fuzz-11#0` |
| P7 | Runtime error model and numerics | 6 | Yes | `literal-escape-decode` if temporal conversions overlap | `algo-compound-interest-decimal#0`, `algo-compound-interest-decimal#4`, `algo-date-daycount-leap#0`, `app-url-shortener#2`, `cluster-clock-duration#4`, `cluster-numerics-decimal#0` |
| P8 | Type surfaces for reads and traversal | 30 | No | `element-loop-semantics`, `lsp-check-diagnostics` | `algo-ackermann#0`, `algo-collatz#0`, `algo-csv-splitter#4`, `algo-date-daycount-leap#1`, `algo-date-daycount-leap#4`, `algo-factorial#2`, `algo-fibonacci#3`, `algo-fizzbuzz#3`, `algo-insertion-sort#0`, `algo-sieve-primes#0`, `app-calendar-scheduler#0`, `app-calendar-scheduler#2`, `app-expression-interpreter#1`, `app-url-shortener#8`, `app-audit-log#1`, `app-library-catalog#1`, `app-double-entry-ledger#0`, `app-dependency-graph#0`, `app-versioned-cms#3`, `app-versioned-cms#5`, `app-dependency-graph#7`, `app-inventory-warehouse#4`, `app-task-tracker#0`, `app-task-tracker#1`, `app-double-entry-ledger#3`, `app-audit-log#4`, `app-audit-log#6`, `cluster-indexes#2`, `cluster-indexes#4`, `cluster-sparse-presence#4` |
| P9 | Local collections | 11 | Yes | `element-loop-semantics`, `feat-defaults` | `algo-collatz#1`, `algo-collatz#2`, `algo-date-daycount-leap#5`, `algo-insertion-sort#1`, `algo-palindrome-utf8#1`, `algo-roman-numerals#4`, `app-calendar-scheduler#4`, `apps:app-ttl-cache#1`, `app-dependency-graph#4`, `app-dependency-graph#5`, `app-audit-log#5` |
| P10 | Saved storage, indexes, presence | 14 | No | `unkeyed-required-fields`, `saved-walk-cursor`, `element-loop-semantics` | `app-calendar-scheduler#1`, `app-mini-spreadsheet#0`, `app-url-shortener#3`, `app-dependency-graph#1`, `cluster-backup-restore#0`, `cluster-clock-duration#3`, `cluster-indexes#1`, `cluster-indexes#3`, `cluster-saved-encoding-integrity#1`, `cluster-sparse-presence#0`, `cluster-sparse-presence#1`, `cluster-sparse-presence#2`, `cluster-sparse-presence#3`, `fuzz-9#1` |
| P11 | Traversal, neighbors, mutation guards | 10 | No | `element-loop-semantics`, `saved-walk-cursor` | `algo-csv-splitter#1`, `algo-csv-splitter#2`, `app-registrar-composite-id#1`, `app-library-catalog#7`, `cluster-controlflow-errors#3`, `cluster-sequences-traversal#0`, `cluster-sequences-traversal#1`, `cluster-sequences-traversal#2`, `cluster-sequences-traversal#3`, `fuzz-9#0` |
| P12 | Enums | 4 | Maybe | `enum-binding-index`, `enum-segment-precision` | `app-expression-interpreter#2`, `app-expression-interpreter#4`, `cluster-enums#2`, `cluster-enums#3` |
| P13 | Data CLI, serve protocol, locks | 5 | No | `saved-walk-cursor`, `cli-doc-migration`, `lsp-retire-inrepo` | `cluster-backup-restore#1`, `cluster-backup-restore#3`, `cluster-serve#0`, `cluster-serve#1`, `cluster-transactions-locks#1` |

Open issue count: 166.

## Design Input Queue

Ask these before implementation reaches the package.

| Package | Question |
|---|---|
| P0 | Should `require ... else` be implemented now, or held as a future language design item? |
| P0 | Should Marrow add a named integer quotient helper/operator? |
| P0 | Should `std::text::split(text, "")` return only scalar pieces or preserve boundary empty strings? |
| P6 | Should conversion builtins implement the broad documented conversions or should unsupported conversions be rejected statically? |
| P6 | Should temporal parse helpers accept common ISO 8601/RFC 3339 boundary text? |
| P7 | Should runtime evaluator faults be catchable by `try/catch`? |
| P9 | Should local sequences/keyed trees support documented subscript/append behavior, or should docs/checker reject it? |
