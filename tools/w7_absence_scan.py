#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional


ROOT = Path(__file__).resolve().parents[1]
SELF = "tools/w7_absence_scan.py"


@dataclass(frozen=True)
class Hit:
    pattern_id: str
    path: str
    line_no: int
    text: str


Allow = Callable[[str, str, Optional[re.Match[str]]], bool]


@dataclass(frozen=True)
class Pattern:
    id: str
    regex: re.Pattern[str]
    allow: Allow
    path_regex: re.Pattern[str] | None = None


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def allowed_backlog(path: str, _text: str, _match: Optional[re.Match[str]] = None) -> bool:
    return path == "ROADMAP.md" or path == SELF


def allow_paths(*paths: str) -> Allow:
    allowed = set(paths)

    def inner(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
        return allowed_backlog(path, text, match) or path in allowed

    return inner


def allow_future_or_paths(*paths: str) -> Allow:
    allowed = set(paths)

    def inner(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
        return allowed_backlog(path, text, match) or path.startswith("docs/future/") or path in allowed

    return inner


def matched_line(text: str, match: Optional[re.Match[str]]) -> str:
    if match is None:
        return ""
    start = text.rfind("\n", 0, match.start()) + 1
    end = text.find("\n", match.start())
    if end == -1:
        end = len(text)
    return text[start:end]


def current_function_name(text: str, match: Optional[re.Match[str]]) -> str | None:
    if match is None:
        return None
    line_start = text.rfind("\n", 0, match.start()) + 1
    line_end = text.find("\n", match.start())
    if line_end == -1:
        line_end = len(text)
    line_code = rust_code_mask(text[:line_end])[line_start:line_end]
    declaration = re.match(r"\s*fn\s+([A-Za-z_]\w*)\s*\(", line_code)
    if declaration:
        return declaration.group(1)

    prefix = text[: match.start()]
    prefix_code = rust_code_mask(prefix)
    declarations = list(re.finditer(r"(?m)^\s*fn\s+([A-Za-z_]\w*)\s*\(", prefix_code))
    if not declarations:
        return None
    last = declarations[-1]
    brace = prefix_code.find("{", last.end(), match.start())
    if brace == -1:
        return None
    depth = 0
    segment = prefix_code[brace:]
    for index, character in enumerate(segment):
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0 and index != len(segment) - 1:
                return None
    if depth <= 0:
        return None
    return last.group(1)


def rust_code_mask(fragment: str) -> str:
    chars = list(fragment)
    i = 0
    while i < len(fragment):
        if fragment.startswith("//", i):
            newline = fragment.find("\n", i + 2)
            end = len(fragment) if newline == -1 else newline
            for index in range(i, end):
                chars[index] = " "
            if newline == -1:
                return "".join(chars)
            i = newline + 1
            continue
        if fragment.startswith("/*", i):
            end = fragment.find("*/", i + 2)
            end = len(fragment) if end == -1 else end + 2
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        raw = re.match(r"r(#+)?\"", fragment[i:])
        if raw:
            hashes = raw.group(1) or ""
            end = fragment.find(f'"{hashes}', i + raw.end())
            end = len(fragment) if end == -1 else end + len(hashes) + 1
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        character = fragment[i]
        if character == '"':
            start = i
            i = start + 1
            while i < len(fragment):
                if fragment[i] == "\\":
                    i += 2
                    continue
                if fragment[i] == '"':
                    i += 1
                    break
                i += 1
            for index in range(start, min(i, len(fragment))):
                chars[index] = " "
            continue
        if character == "'":
            end = None
            if i + 3 < len(fragment) and fragment[i + 1] == "\\" and fragment[i + 3] == "'":
                end = i + 4
            elif i + 2 < len(fragment) and fragment[i + 2] == "'":
                end = i + 3
            if end is None:
                i += 1
                continue
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        i += 1
    return "".join(chars)


def in_function_context(text: str, match: Optional[re.Match[str]], names: tuple[str, ...]) -> bool:
    return current_function_name(text, match) in names


def allow_underscore_concat(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    line = matched_line(text, match).lower()
    return (
        path == "crates/marrow-syntax/tests/cases/parse_expressions.rs"
        and "const bad" in line
        and " _ " in line
    )


def allow_at_rejection_owner(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    return allowed_backlog(path, text, match)


def allow_write_builtin(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    if match and match.group(0) in {"CheckedBuiltinCall::Write", "OutputKind::Write"}:
        return False
    if match:
        code = rust_code_mask(text[: match.end()])[match.start() : match.end()]
        if not code.strip():
            return False
    if path.startswith("__probe__/") or path.endswith(".mw"):
        return False
    if path.startswith("docs/"):
        return False
    if match and match.group(0).lstrip().startswith(r"\n"):
        return False
    return True


def allow_loop_label(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    if path.startswith("__probe__/") or path.endswith(".mw"):
        return False
    line = matched_line(text, match).lstrip()
    if line.startswith(r"\n"):
        return False
    if path == "crates/marrow-syntax/src/parse_decl/stmt.rs":
        return "self.recover_removed_loop_label();" in line or in_function_context(
            text, match, ("recover_removed_loop_label",)
        )
    if path == "crates/marrow-syntax/src/parse_decl/statement_lines.rs":
        return "UnsupportedSyntax::LoopLabels" in line
    if path == "crates/marrow-syntax/src/diagnostic.rs":
        return line.strip() == "LoopLabels,"
    if path == "crates/marrow-syntax/tests/cases/parse_control_flow.rs":
        return in_function_context(
            text,
            match,
            (
                "loop_labels_are_rejected_as_removed_syntax",
                "labeled_break_and_continue_are_rejected_as_removed_syntax",
            ),
        )
    return False


MODE_TEST_CONTEXTS = (
    "removed_parameter_modes_are_rejected",
    "out_and_inout_parse_as_ordinary_parameter_names",
    "removed_call_argument_modes_are_rejected",
    "inout_parameter_and_argument_syntax_is_rejected_before_runtime",
    "out_and_inout_parse_as_ordinary_names",
    "out_and_inout_can_head_ordinary_call_argument_expressions",
    "check_rejects_removed_inout_syntax_for_a_project_directory",
    "parameters_are_read_only_but_values_can_be_returned",
)


def allow_removed_mode(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    return path in {
        "crates/marrow-syntax/tests/cases/parse_paths_calls.rs",
        "crates/marrow-syntax/tests/cases/parse_types_params.rs",
        "crates/marrow-run/tests/cases/eval_vars_return_values.rs",
        "crates/marrow/tests/cases/check_cli.rs",
    } and in_function_context(text, match, MODE_TEST_CONTEXTS)


def allow_quoted_segments(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    if path.startswith("docs/future/"):
        return True
    line = matched_line(text, match)
    if path == "crates/marrow-syntax/src/parse_expr.rs":
        return "QuotedFieldSegments" in line or "quoted field segments are not part" in line
    if path == "crates/marrow-syntax/src/diagnostic.rs":
        return line.strip() == "QuotedFieldSegments,"
    if path == "crates/marrow-syntax/tests/cases/parse_paths_calls.rs":
        return in_function_context(
            text,
            match,
            (
                "quoted_field_segments_are_parse_errors",
                "quoted_keyword_field_name_reports_a_parse_error",
                "unterminated_quoted_field_segment_does_not_panic",
            ),
        )
    if path in {
        "docs/language/grammar.md",
        "docs/language/resources-and-storage.md",
    }:
        lowered = line.lower()
        return "quoted field segments" in lowered and "operator" in lowered
    return False


def allow_evidence_fields(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    if path != "crates/marrow/src/backup/archive.rs":
        return False
    line = matched_line(text, match)
    return (
        current_function_name(text, match) == "commit_json_omits_activation_receipt_payloads"
        and "activation_field(\"" in line
    )


def allow_glob_grammar(path: str, text: str, match: Optional[re.Match[str]] = None) -> bool:
    if allowed_backlog(path, text, match):
        return True
    line = matched_line(text, match)
    if path == "docs/project-config.md":
        return "glob metacharacters" in line or "every `.mw` file" in line
    if path == "crates/marrow-project/tests/cases/config.rs":
        return current_function_name(text, match) == "rejects_test_entries_with_glob_metacharacters"
    if path == "crates/marrow-project/tests/cases/discovery.rs":
        return current_function_name(text, match) == "star_test_entry_is_invalid_config"
    return False


PATTERNS: tuple[Pattern, ...] = (
    Pattern(
        "serve",
        re.compile(r"\bmarrow\s+serve\b|\b(?:Command|Subcommand)::Serve\b"),
        allow_paths("crates/marrow/tests/cases/usage_cli.rs"),
        re.compile(r"docs/(?:serve-protocol|implementation/serve-lsp)\.md"),
    ),
    Pattern(
        "lsp",
        re.compile(r"\bmarrow\s+lsp\b|\b(?:Command|Subcommand)::Lsp\b"),
        allow_paths("crates/marrow/tests/cases/usage_cli.rs"),
        re.compile(r"docs/(?:lsp|implementation/serve-lsp)\.md"),
    ),
    Pattern(
        "protocol-code-family",
        re.compile(r"(?<![A-Za-z0-9_])protocol\.[a-z][a-z0-9_.]*"),
        allow_paths(),
    ),
    Pattern(
        "at-sugar",
        re.compile(r"(?m)^\s*resource\s+[A-Z]\w*\s+at\b|`resource \{name\} at|\bKeyword::At\b|\"at\"\s*=>\s*Keyword::At\b"),
        allow_at_rejection_owner,
    ),
    Pattern(
        "underscore-concat",
        re.compile(r"(?<![A-Za-z0-9_])(?:\"[^\"\n]*\"|[A-Za-z_]\w*\([^)\n]*\)|[A-Za-z_]\w*\.[A-Za-z_]\w*|(?!(?:for|let|var)\b)[A-Za-z_]\w*)\s+_\s+(?:\"[^\"\n]*\"|[A-Za-z_]\w*|\(|\[)"),
        allow_underscore_concat,
    ),
    Pattern(
        "write-builtin",
        re.compile(r"CheckedBuiltinCall::Write|OutputKind::Write|^\s*write\s*\(|\\n\s*write\s*\(", re.MULTILINE),
        allow_write_builtin,
    ),
    Pattern(
        "finally-control-flow",
        re.compile(r"\btry\b[\s\S]{0,300}\bfinally\b|check\.finally_control_flow|TryFinally|FinallyClause"),
        allow_paths(
            "crates/marrow-syntax/tests/cases/parse_control_flow.rs",
            "crates/marrow-syntax/tests/cases/parse_statements.rs",
        ),
    ),
    Pattern(
        "loop-labels",
        re.compile(r"(?m)^\s*[A-Za-z_]\w*:\s*(?:while|for)\b|^\s*(?:break|continue)[ \t]+[A-Za-z_]\w*\b|\\n\s*(?:break|continue)[ \t]+[A-Za-z_]\w*\b|loop_label|try_loop_label|format_label|LoopLabels"),
        allow_loop_label,
    ),
    Pattern(
        "out-mode",
        re.compile(r"(?m)\bKeyword::Out\b|\"out\"\s*=>|(?:^\s*|[,(]\s*)out\s+(?!of\b)[A-Za-z_]\w*\b|\bout\s+(?!of\b)[A-Za-z_]\w*\s*:|\bout\s+\^|Checked(?:Arg|Param)Mode::Out"),
        allow_removed_mode,
    ),
    Pattern(
        "inout-mode",
        re.compile(r"(?m)\bKeyword::Inout\b|\"inout\"\s*=>|(?:^\s*|[,(]\s*)inout\s+[A-Za-z_]\w*\b|\binout\s+[A-Za-z_]\w*\s*:|\binout\s+\^|Checked(?:Arg|Param)Mode::Inout"),
        allow_removed_mode,
    ),
    Pattern(
        "decimal-ranges",
        re.compile(r"RangeIter::Decimal|decimal_range_iter|ScalarType::Decimal.*is_steppable|\bfor\s+\w+\s+in\s+[-+]?\d+\.\d+\s*\.\.=?\s*[-+]?\d+\.\d+"),
        allow_paths(),
    ),
    Pattern(
        "quoted-segments",
        re.compile(r"(?<!\.)\.\"[A-Za-z_][^\"\n]*\"|QuotedFieldSegments|quoted field segments"),
        allow_quoted_segments,
    ),
    Pattern(
        "map-sugar",
        re.compile(r"\bmap\s*\[[^\]]+\]|keyed-leaf-map|map\[K,\s*V\]"),
        allow_future_or_paths(
            "tools/docs_lint.py",
        ),
    ),
    Pattern(
        "check-data",
        re.compile(r"\bcheck\s+--data\b|CheckData|cmd_check_data"),
        allowed_backlog,
    ),
    Pattern(
        "single-file-check",
        re.compile(r"\bmarrow\s+check\s+[^`\n]*\.mw\b|single_file_check"),
        allowed_backlog,
    ),
    Pattern(
        "completion-dir",
        re.compile(r"(?:^|/)completion/[^/\s]+\.rs\b|\bmod\s+completion\b|completion::(?:default|index|retire|transform|receipt)"),
        allow_paths(),
        re.compile(r"(?:^|/)completion/[^/\s]+\.rs\b"),
    ),
    Pattern(
        "rebind-resume",
        re.compile(r"rebind_activation_resume_program|resume_completion|evolve_cli_resume|\.resume\s*\("),
        allow_paths(
            "crates/marrow-check/tests/cases/project_schema.rs",
            "docs/error-codes.md",
        ),
    ),
    Pattern("touches-saved-data", re.compile(r"\btouches_saved_data\b"), allow_paths()),
    Pattern("future-ephemeral-root-effects", re.compile(r"\b(?:FutureEphemeralRootEffects|EphemeralRootEffects)\b"), allow_paths()),
    Pattern("match-fields", re.compile(r"\bMatchFields\b|\bmatch_fields\b|\bMatch fields\b"), allow_paths()),
    Pattern(
        "savepoint-journal",
        re.compile(r"\bSavepoint\b|\bsavepoint\b|Vec<Undo>|UndoJournal|journal sink|bounded journal|lower_savepoint_level"),
        allow_future_or_paths("docs/language/syntax.md"),
    ),
    Pattern(
        "meta-cells-01-03",
        re.compile(r"MetaCell::(?:AcceptedCatalog|SourceDigest|StateDigest|EngineProfile|Activation|Receipt)|MetaCell::[A-Za-z_]\w*\s*=>\s*0x0[1-3]\b"),
        allow_paths(),
    ),
    Pattern(
        "evidence-fields",
        re.compile(r"_retire_evidence_digest|_default_records_by_id|_records_(?:backfilled|retired|transformed)|_indexes_rebuilt|ActivationEvidence|CommitEvidence|ReceiptEvidence|per-effect .*metadata"),
        allow_evidence_fields,
    ),
    Pattern(
        "acceptedCatalog-config",
        re.compile(r"acceptedCatalog|ConfigPathField::AcceptedCatalog|accepted_catalog_path"),
        allow_paths("crates/marrow-project/tests/cases/config.rs"),
    ),
    Pattern(
        "tempfile-crate",
        re.compile(r"\btempfile\b|tempfile::|NamedTempFile"),
        allow_paths(),
    ),
    Pattern(
        "glob-grammar",
        re.compile(r"\*\*/|/\*\.mw|\*\.mw|\btest_pattern_base\b|\bglob(?:set|walk)?::"),
        allow_glob_grammar,
    ),
    Pattern(
        "drift-codes",
        re.compile(r"evolve\.(?:store_commit_drift|plan_mismatch|witness_drift)"),
        allow_paths(),
    ),
    Pattern(
        "capability-kind",
        re.compile(r"capability kind|kind row|kind-style|capability_kind|CapabilityKind|\"kind\"\s*:\s*\"capability\""),
        allow_paths(),
    ),
    Pattern(
        "fictional-helpers",
        re.compile(r"loadBookId|loadEnrollmentId|fictional helper|fictional .*then.*as"),
        allow_paths(),
    ),
)


PROBES: dict[str, str] = {
    "docs/serve-protocol.md": "# old serve docs\n",
    "docs/lsp.md": "# old lsp docs\n",
    "__probe__/protocol.txt": "protocol.request\n",
    "__probe__/at.mw": "module seed\nresource Book at ^books(id: int): Book\n",
    "__probe__/concat.mw": "module m\nfn f(): string\n    return first _ last\n",
    "__probe__/write.mw": "module m\nfn f()\n    write(\"hello\")\n",
    "crates/marrow-check/src/write_removed_probe.rs": "CheckedBuiltinCall::Write\nOutputKind::Write\n",
    "crates/marrow/tests/cases/raw_string_write_probe.rs": 'let source = r#"\nmodule m\nfn f()\n    write("hello")\n"#;\n',
    "__probe__/finally.mw": "module m\nfn f()\n    try\n        return\n    finally\n        return\n",
    "__probe__/labels.mw": "module m\nfn f()\n    break outer\n",
    "__probe__/out.mw": "module m\nfn f()\n    out result\n",
    "__probe__/inout.mw": "module m\nfn f()\n    inout total\n",
    "__probe__/decimal.mw": "module m\nfn f()\n    for n in 1.0..2.0\n        return\n",
    "__probe__/quoted.mw": "module m\nfn f()\n    return thing.\"field\"\n",
    "__probe__/map.mw": "module m\nfn f(rows: map[string, int])\n    return\n",
    "__probe__/check-data.txt": "marrow check --data\n",
    "crates/marrow/tests/cases/ordinary_check_data_probe.rs": "ordinary marrow check --data\n",
    "__probe__/single-file.txt": "marrow check app.mw\n",
    "crates/marrow/tests/cases/ordinary_single_file_probe.rs": "ordinary marrow check app.mw\n",
    "crates/marrow-syntax/tests/cases/ordinary_concat_probe.rs": "ordinary first _ last\n",
    "crates/marrow-check/src/evolution/completion/default.rs": "pub fn old() {}\n",
    "__probe__/resume.rs": "rebind_activation_resume_program();\n",
    "__probe__/touch.rs": "touches_saved_data\n",
    "__probe__/future.rs": "FutureEphemeralRootEffects\n",
    "__probe__/match.rs": "MatchFields\n",
    "__probe__/savepoint.rs": "Savepoint\n",
    "__probe__/meta.rs": "MetaCell::Commit => 0x01\n",
    "__probe__/evidence.rs": "_records_backfilled\n",
    "__probe__/catalog.json": "acceptedCatalog\n",
    "__probe__/tempfile.rs": "tempfile::tempdir()\n",
    "__probe__/glob.txt": "**/*.mw\n",
    "__probe__/drift.txt": "evolve.witness_drift\n",
    "__probe__/capability.txt": "capability kind\n",
    "__probe__/fictional.txt": "loadBookId\n",
}

EXPECTED_PROBE_IDS = {pattern.id for pattern in PATTERNS}
PROBE_EXPECTATIONS: dict[str, set[str]] = {
    "crates/marrow-check/src/write_removed_probe.rs": {"write-builtin"},
    "crates/marrow/tests/cases/raw_string_write_probe.rs": {"write-builtin"},
    "crates/marrow/tests/cases/ordinary_check_data_probe.rs": {"check-data"},
    "crates/marrow/tests/cases/ordinary_single_file_probe.rs": {"single-file-check"},
    "crates/marrow-syntax/tests/cases/ordinary_concat_probe.rs": {"underscore-concat"},
}

REVIEW_PROBES: dict[str, tuple[set[str], str]] = {
    "crates/marrow/src/backup/archive.rs": (
        {"evidence-fields"},
        'activation_field("_records_backfilled")\n',
    ),
    "crates/marrow-project/src/lib.rs": (
        {"glob-grammar"},
        '"**/*.mw"\nglobset::Glob::new("tests/*.mw")\n',
    ),
    "crates/marrow-syntax/src/parse_decl/stmt.rs": (
        {"loop-labels"},
        "try_loop_label()\n",
    ),
    "crates/marrow-syntax/src/parse_decl/params.rs": (
        {"out-mode", "inout-mode"},
        "Keyword::Out\nCheckedParamMode::Inout\n",
    ),
    "crates/marrow-syntax/src/parse_expr.rs": (
        {"quoted-segments"},
        'thing."field"\n',
    ),
}


def scan_paths() -> list[Path]:
    roots = [ROOT / "crates", ROOT / "docs", ROOT / "fixtures", ROOT / "tools"]
    roots.extend(path for path in ROOT.glob("*.md"))
    roots.extend([ROOT / "Cargo.toml", ROOT / "Cargo.lock"])

    files: list[Path] = []
    for root in roots:
        if root.is_file():
            files.append(root)
        elif root.exists():
            files.extend(path for path in root.rglob("*") if path.is_file())
    return sorted(set(files))


def read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return None


def line_no(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def collect_text_hits(texts: list[tuple[str, str]]) -> list[Hit]:
    hits: list[Hit] = []
    for path, text in texts:
        for pattern in PATTERNS:
            if pattern.path_regex and pattern.path_regex.search(path):
                if not pattern.allow(path, text, None):
                    hits.append(Hit(pattern.id, path, 1, f"path:{path}"))
            for match in pattern.regex.finditer(text):
                if pattern.allow(path, text, match):
                    continue
                snippet = text[match.start() : text.find("\n", match.start())]
                hits.append(Hit(pattern.id, path, line_no(text, match.start()), snippet.strip()))
    return hits


def collect_hits(extra: dict[str, str] | None = None) -> list[Hit]:
    texts: list[tuple[str, str]] = []
    for path in scan_paths():
        relative = rel(path)
        text = read_text(path)
        if text is not None:
            texts.append((relative, text))
    if extra:
        texts.extend(extra.items())
    return collect_text_hits(texts)


def seed_text(seed: str) -> str:
    if seed != "at-sugar":
        raise SystemExit(f"unknown seed {seed!r}")
    return "module seed\nresource Book at ^books(id: int): Book\n"


def print_hits(hits: list[Hit]) -> None:
    for hit in hits:
        print(f"{hit.path}:{hit.line_no}: {hit.pattern_id}: {hit.text}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", choices=["at-sugar"])
    parser.add_argument("--assert-seed", choices=["at-sugar"])
    parser.add_argument("--assert-probes", action="store_true")
    args = parser.parse_args()

    if args.assert_probes:
        hits = collect_hits(PROBES)
        real_hits = [hit for hit in hits if not hit.path.startswith("__probe__/") and hit.path not in PROBES]
        if real_hits:
            print_hits(real_hits)
            return 1
        seen = {hit.pattern_id for hit in hits if hit.path.startswith("__probe__/") or hit.path in PROBES}
        missing = sorted(EXPECTED_PROBE_IDS - seen)
        if missing:
            print(f"w7_absence_scan probe suite missed: {', '.join(missing)}")
            print_hits([hit for hit in hits if hit.path.startswith("__probe__/") or hit.path in PROBES])
            return 1
        missing_probe_paths = []
        for path, expected_ids in PROBE_EXPECTATIONS.items():
            seen_for_path = {hit.pattern_id for hit in hits if hit.path == path}
            missing_for_path = sorted(expected_ids - seen_for_path)
            if missing_for_path:
                missing_probe_paths.append(f"{path}: {', '.join(missing_for_path)}")
        if missing_probe_paths:
            print("w7_absence_scan probe suite missed reviewed false-negative probes:")
            for missing_path in missing_probe_paths:
                print(missing_path)
            print_hits([hit for hit in hits if hit.path in PROBES])
            return 1
        review_texts = [(path, text) for path, (_expected, text) in REVIEW_PROBES.items()]
        review_hits = collect_text_hits(review_texts)
        missing_review_probes = []
        for path, (expected_ids, _text) in REVIEW_PROBES.items():
            seen_for_path = {hit.pattern_id for hit in review_hits if hit.path == path}
            missing_for_path = sorted(expected_ids - seen_for_path)
            if missing_for_path:
                missing_review_probes.append(f"{path}: {', '.join(missing_for_path)}")
        if missing_review_probes:
            print("w7_absence_scan probe suite missed exact-path review probes:")
            for missing_path in missing_review_probes:
                print(missing_path)
            print_hits(review_hits)
            return 1
        print(f"w7_absence_scan probe suite detected {len(EXPECTED_PROBE_IDS)} pattern families")
        return 0

    extra = None
    expected_seed = args.seed or args.assert_seed
    if expected_seed:
        extra = {"__seed__/w7_absence_seed.mw": seed_text(expected_seed)}

    hits = collect_hits(extra)
    if args.assert_seed:
        seed_hits = [hit for hit in hits if hit.path.startswith("__seed__/")]
        real_hits = [hit for hit in hits if not hit.path.startswith("__seed__/")]
        if real_hits:
            print_hits(real_hits)
            return 1
        if len(seed_hits) == 1 and seed_hits[0].pattern_id == args.assert_seed:
            print(
                f"w7_absence_scan seed {args.assert_seed} detected "
                f"{seed_hits[0].path}:{seed_hits[0].line_no}"
            )
            return 0
        print_hits(seed_hits)
        print(f"seed {args.assert_seed} was not detected exactly once")
        return 1

    if hits:
        print_hits(hits)
        return 1

    print("w7_absence_scan passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
