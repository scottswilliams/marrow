#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SELF = "tools/cleanup_scan.py"


@dataclass(frozen=True)
class Hit:
    family: str
    path: str
    line_no: int
    text: str


@dataclass(frozen=True)
class Family:
    id: str
    kind: str
    description: str


FAMILIES = {
    "unsafe-rust": Family("unsafe-rust", "zero-hit", "unsafe Rust in tracked source"),
    "broad-allow": Family("broad-allow", "report", "broad allow attributes"),
    "compatibility-terms": Family("compatibility-terms", "report", "retired-surface terms"),
    "test-size": Family("test-size", "report", "oversized Rust test files"),
    "production-panic": Family("production-panic", "report", "panic-like production calls"),
    "semantic-prose-assertion": Family(
        "semantic-prose-assertion",
        "lane-owned",
        "rendered-prose assertions outside CLI/rendering boundaries",
    ),
    "raw-store-test-hook": Family(
        "raw-store-test-hook",
        "lane-owned",
        "test/support raw store mutation hooks",
    ),
}


PROBES = {
    "unsafe-rust": [("__probe__/unsafe.rs", "unsafe { call(); }\n")],
    "broad-allow": [("__probe__/allow.rs", "#[allow(clippy::too_many_arguments)]\nfn f() {}\n")],
    "compatibility-terms": [("__probe__/legacy.rs", "fn legacy_shape() {}\n")],
    "test-size": [("crates/marrow/tests/probe.rs", "\n".join("#[test]\nfn t() {}" for _ in range(31)))],
    "production-panic": [("crates/marrow/src/probe.rs", "fn f() { value.expect(\"present\"); }\n")],
    "semantic-prose-assertion": [
        (
            "crates/marrow-check/tests/cases/project_probe.rs",
            "fn t() { assert!(diagnostic.message.contains(\"text\")); }\n",
        )
    ],
    "raw-store-test-hook": [
        (
            "crates/marrow/tests/support_data/probe.rs",
            "fn t() { store.write_node(path); TreeStore::write_data_value(); }\n",
        )
    ],
}


def rust_code_mask(text: str) -> str:
    chars = list(text)
    i = 0
    while i < len(text):
        if text.startswith("//", i):
            end = text.find("\n", i + 2)
            end = len(text) if end == -1 else end
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        if text.startswith("/*", i):
            end = text.find("*/", i + 2)
            end = len(text) if end == -1 else end + 2
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        raw = re.match(r"r(#+)?\"", text[i:])
        if raw:
            hashes = raw.group(1) or ""
            end = text.find(f'"{hashes}', i + raw.end())
            end = len(text) if end == -1 else end + len(hashes) + 1
            for index in range(i, end):
                chars[index] = " "
            i = end
            continue
        if text[i] == '"':
            start = i
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            for index in range(start, min(i, len(text))):
                chars[index] = " "
            continue
        i += 1
    return "".join(chars)


def line_no(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def matched_line(text: str, offset: int) -> str:
    start = text.rfind("\n", 0, offset) + 1
    end = text.find("\n", offset)
    if end == -1:
        end = len(text)
    return text[start:end].strip()


def tracked_files() -> list[str]:
    output = subprocess.check_output(
        ["git", "-C", str(ROOT), "ls-files"],
        text=True,
    )
    return [path for path in sorted(output.splitlines()) if path != SELF]


def read_tracked_text(path: str) -> str | None:
    try:
        return (ROOT / path).read_text(encoding="utf-8")
    except (FileNotFoundError, UnicodeDecodeError):
        return None


def text_entries(extra: dict[str, str] | None) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []
    for path in tracked_files():
        text = read_tracked_text(path)
        if text is not None:
            entries.append((path, text))
    if extra:
        entries.extend(sorted(extra.items()))
    return entries


def scan_unsafe(path: str, text: str) -> list[Hit]:
    if not path.endswith(".rs"):
        return []
    code = rust_code_mask(text)
    return [
        Hit("unsafe-rust", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in re.finditer(r"\bunsafe\b", code)
    ]


def scan_broad_allow(path: str, text: str) -> list[Hit]:
    if not path.endswith(".rs"):
        return []
    pattern = re.compile(r"#\[allow\((?:clippy::too_many_arguments|dead_code|unused|warnings)")
    return [
        Hit("broad-allow", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in pattern.finditer(text)
    ]


def scan_compatibility_terms(path: str, text: str) -> list[Hit]:
    allowed_paths = {
        "docs/data-evolution.md",
        "docs/future/data-evolution.md",
        "docs/tooling-surfaces.md",
        "docs/language/resources-and-storage.md",
        "docs/implementation/store.md",
    }
    if path in allowed_paths or path.startswith("docs/future/"):
        return []
    pattern = re.compile(
        r"\blegacy[_-]|\blegacy order-sensitive\b|\bprototype\b|\bshim\b|"
        r"\btest_only\b|\bback-compatible\b|\bBack-compatible\b"
    )
    return [
        Hit("compatibility-terms", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in pattern.finditer(text)
    ]


def scan_test_size(path: str, text: str) -> list[Hit]:
    if not (path.endswith(".rs") and "/tests/" in path):
        return []
    line_count = text.count("\n") + (0 if text.endswith("\n") else 1)
    test_count = len(re.findall(r"(?m)^\s*#\[(?:tokio::)?test\]", text))
    hits = []
    if line_count > 500:
        hits.append(Hit("test-size", path, 1, f"{line_count} lines"))
    if test_count > 30:
        hits.append(Hit("test-size", path, 1, f"{test_count} tests"))
    return hits


def strip_test_modules(code: str) -> str:
    masked = code
    for match in re.finditer(r"(?m)^\s*#\[cfg\(test\)\]\s*\n\s*mod\s+\w+\s*\{", code):
        start = match.start()
        brace = code.find("{", match.end() - 1)
        depth = 0
        end = len(code)
        for index in range(brace, len(code)):
            if code[index] == "{":
                depth += 1
            elif code[index] == "}":
                depth -= 1
                if depth == 0:
                    end = index + 1
                    break
        masked = masked[:start] + (" " * (end - start)) + masked[end:]
    return masked


def scan_production_panic(path: str, text: str) -> list[Hit]:
    if not (path.startswith("crates/") and "/src/" in path and path.endswith(".rs")):
        return []
    code = strip_test_modules(rust_code_mask(text))
    pattern = re.compile(r"\.(?:unwrap|expect)\s*\(|\bpanic!\s*\(|\bunreachable!\s*\(")
    return [
        Hit("production-panic", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in pattern.finditer(code)
    ]


def scan_semantic_prose(path: str, text: str) -> list[Hit]:
    if not (path.startswith("crates/") and "/tests/" in path and path.endswith(".rs")):
        return []
    if path.startswith("crates/marrow/tests/") or path.startswith("crates/marrow-syntax/tests/"):
        return []
    code = rust_code_mask(text)
    pattern = re.compile(
        r"(?:message|stderr|stdout|to_string\(\)|from_utf8_lossy)[A-Za-z0-9_().&\s!]*\.contains\s*\(",
        re.MULTILINE,
    )
    return [
        Hit("semantic-prose-assertion", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in pattern.finditer(code)
    ]


def scan_raw_store_hook(path: str, text: str) -> list[Hit]:
    if not ("/tests/" in path or "support" in path):
        return []
    pattern = re.compile(
        r"\bwrite_raw_[A-Za-z0-9_]*\b|\braw_catalog\b|"
        r"\bTreeStore::write_data_value\b|\bwrite_node\s*\(|\bdelete_data_subtree\s*\("
    )
    return [
        Hit("raw-store-test-hook", path, line_no(text, match.start()), matched_line(text, match.start()))
        for match in pattern.finditer(text)
    ]


SCANNERS = {
    "unsafe-rust": scan_unsafe,
    "broad-allow": scan_broad_allow,
    "compatibility-terms": scan_compatibility_terms,
    "test-size": scan_test_size,
    "production-panic": scan_production_panic,
    "semantic-prose-assertion": scan_semantic_prose,
    "raw-store-test-hook": scan_raw_store_hook,
}


def collect_hits(families: set[str], extra: dict[str, str] | None = None) -> list[Hit]:
    hits: list[Hit] = []
    for path, text in text_entries(extra):
        for family in families:
            hits.extend(SCANNERS[family](path, text))
    return sorted(hits, key=lambda hit: (hit.family, hit.path, hit.line_no, hit.text))


def print_summary(hits: list[Hit], families: set[str]) -> None:
    counts = {family: 0 for family in families}
    for hit in hits:
        counts[hit.family] += 1
    for family in sorted(families):
        print(f"{family}: {counts[family]}")
    for hit in hits[:50]:
        print(f"{hit.path}:{hit.line_no}: {hit.family}: {hit.text}")
    if len(hits) > 50:
        print(f"... {len(hits) - 50} more hit(s)")


def assert_probes() -> int:
    failed = False
    for family, probes in PROBES.items():
        probe_hits = collect_hits({family}, dict(probes))
        seen = [hit for hit in probe_hits if hit.path.startswith("__probe__/") or "probe.rs" in hit.path]
        if not seen:
            print(f"cleanup_scan probe missed {family}")
            failed = True
    original_tracked_files = globals()["tracked_files"]
    globals()["tracked_files"] = lambda: ["__probe__/deleted.rs"]
    deleted_hits = collect_hits({"unsafe-rust"})
    globals()["tracked_files"] = original_tracked_files
    if deleted_hits:
        print("cleanup_scan missing tracked file probe produced hits")
        failed = True
    if failed:
        return 1
    print(f"cleanup_scan probe suite detected {len(PROBES)} family probes")
    return 0


def strictable(family: str) -> bool:
    return FAMILIES[family].kind in {"zero-hit", "lane-owned"}


def selected_families(names: list[str] | None) -> set[str]:
    if not names:
        return set(FAMILIES)
    unknown = sorted(set(names) - set(FAMILIES))
    if unknown:
        raise SystemExit(f"unknown family: {', '.join(unknown)}")
    return set(names)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assert-probes", action="store_true")
    parser.add_argument("--summary", action="store_true")
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--family", action="append")
    args = parser.parse_args()

    if args.assert_probes:
        return assert_probes()

    families = selected_families(args.family)
    if args.strict and not args.family:
        print("--strict requires at least one --family")
        return 2
    if args.strict:
        invalid = sorted(family for family in families if not strictable(family))
        if invalid:
            print(
                "--strict is only enabled for zero-hit or lane-owned families; "
                f"not enabled for: {', '.join(invalid)}"
            )
            return 2

    hits = collect_hits(families)
    if args.summary or not args.strict:
        print_summary(hits, families)
    return 1 if args.strict and hits else 0


if __name__ == "__main__":
    sys.exit(main())
