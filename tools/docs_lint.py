from pathlib import Path
import re
import sys

import removed_surface_scan


ROOT = Path(__file__).resolve().parents[1]


def read_text(path):
    return path.read_text(encoding="utf-8")


def docs_files():
    roots = [ROOT / "docs"]
    for name in ("README.md", "AGENTS.md"):
        path = ROOT / name
        if path.exists():
            roots.append(path)

    files = []
    for root in roots:
        if root.is_file():
            files.append(root)
        elif root.exists():
            files.extend(path for path in root.rglob("*.md") if path.is_file())
    return sorted(files)


def rel(path):
    return path.relative_to(ROOT).as_posix()


def has_phrase(text, phrase):
    normalized_text = " ".join(text.replace("`", "").split())
    normalized_phrase = " ".join(phrase.split())
    return normalized_phrase in normalized_text


def check_data_evolution_outcome_names(path, text, failures):
    canonical = [
        "NoOp",
        "CatalogOnly",
        "IndexDropped",
        "DataProof",
        "Default",
        "DerivedRebuild",
        "Transform",
        "DestructiveDecisionRequired",
        "RepairRequired",
    ]
    if rel(path) == "docs/implementation/check/evolution.md":
        missing = [name for name in canonical if f"`{name}`" not in text]
        if missing:
            failures.append(
                f"{rel(path)} missing canonical evolution outcome names: {', '.join(missing)}"
            )

    aliases = {
        r"\bCompatibilityLensRequired\b": "use Default when naming the current verdict",
        r"\bTypedTransformRequired\b": "use Transform when naming the current verdict",
        r"\bEngineRecompileRequired\b": "this outcome no longer exists in the current verdict",
        r"`Nothing`": "use NoOp when naming the current verdict",
        r"`Approve`": "use DestructiveDecisionRequired when naming the current blocking verdict",
        r"\bNo Op\b": "use NoOp when naming the internal outcome",
        r"\bIndex Rebuild\b": "use DerivedRebuild when naming the internal outcome",
        r"\bData Proof\b": "use DataProof when naming the internal outcome",
        r"\bCatalog Only\b": "use CatalogOnly when naming the internal outcome",
        r"\bCompatibility Lens Required\b": (
            "use Default when naming the current verdict"
        ),
        r"\bDerived Rebuild\b": "use DerivedRebuild when naming the internal outcome",
        r"\bTyped Transform Required\b": (
            "use Transform when naming the current verdict"
        ),
        r"\bDestructive Decision Required\b": (
            "use DestructiveDecisionRequired when naming the internal outcome"
        ),
        r"\bRepair Required\b": "use RepairRequired when naming the internal outcome",
    }
    for pattern, reason in aliases.items():
        if re.search(pattern, text):
            failures.append(f"{rel(path)} contains non-canonical outcome name {pattern!r}: {reason}")


def keyword_spellings(failures):
    path = ROOT / "crates" / "marrow-syntax" / "src" / "token.rs"
    text = read_text(path)
    match = re.search(
        r"pub\(crate\) fn keyword\(text: &str\) -> Option<Keyword> \{\s*"
        r"Some\(match text \{(?P<body>.*?)^\s*_ => return None,",
        text,
        flags=re.DOTALL | re.MULTILINE,
    )
    if not match:
        failures.append("could not find marrow-syntax keyword() match table")
        return []
    arms = re.findall(
        r'^\s*"([^"]+)"\s*=>\s*Keyword::[A-Za-z0-9_]+,\s*$',
        match.group("body"),
        flags=re.MULTILINE,
    )
    if not arms:
        failures.append("could not read any marrow-syntax keyword() arms")
    return arms


def documented_reserved_words(failures):
    path = ROOT / "docs" / "language" / "syntax.md"
    text = read_text(path)
    anchor = "Marrow parser-reserved words are:\n\n```text\n"
    start = text.find(anchor)
    if start == -1:
        failures.append("docs/language/syntax.md missing parser-reserved words block")
        return []
    start += len(anchor)
    end = text.find("\n```", start)
    if end == -1:
        failures.append("docs/language/syntax.md parser-reserved words block is unterminated")
        return []
    return text[start:end].split()


def check_parser_reserved_words(failures):
    expected = keyword_spellings(failures)
    actual = documented_reserved_words(failures)
    if not expected or not actual or actual == expected:
        return

    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    mismatch = next(
        (
            f"position {index + 1}: expected {want!r}, found {got!r}"
            for index, (want, got) in enumerate(zip(expected, actual))
            if want != got
        ),
        None,
    )
    if mismatch is None and len(expected) != len(actual):
        mismatch = f"length mismatch: expected {len(expected)} words, found {len(actual)}"

    details = []
    if missing:
        details.append(f"missing: {', '.join(missing)}")
    if extra:
        details.append(f"extra: {', '.join(extra)}")
    if mismatch:
        details.append(mismatch)
    failures.append(
        "docs/language/syntax.md parser-reserved words must match "
        f"marrow-syntax keyword() order ({'; '.join(details)})"
    )


def main():
    failures = []

    for path in (ROOT / "docs" / "superpowers", ROOT / "docs" / "roadmap"):
        if path.exists():
            failures.append(f"{rel(path)} must not exist")

    forbidden = {
        "docs/superpowers": "process docs were evacuated",
        "docs/roadmap": "process docs were evacuated",
        "Release Package Shape": "release-shape notes do not belong in install docs",
        r"target\release\marrow.exe": "Windows release-path claim is not shipped",
        "store-vs-typed-source": "future data tools are state-vs-state",
        "store-vs-source": "future data tools are state-vs-state",
    }

    for path in docs_files():
        text = read_text(path)
        for needle, reason in forbidden.items():
            if needle in text:
                failures.append(f"{rel(path)} contains {needle!r}: {reason}")
        check_data_evolution_outcome_names(path, text, failures)
    check_parser_reserved_words(failures)

    install = ROOT / "docs" / "install.md"
    if install.exists():
        text = read_text(install)
        if "cargo install --locked --path crates/marrow" not in text:
            failures.append("docs/install.md must install with --locked")
        if re.search(r"cargo\s+install\s+--path\s+crates/marrow", text):
            failures.append("docs/install.md has an unlocked cargo install command")
    else:
        failures.append("docs/install.md is missing")

    control_flow = ROOT / "docs" / "future" / "language" / "control-flow-and-effects.md"
    if control_flow.exists() and "finally rules" in read_text(control_flow):
        failures.append("future require...else sketch must not mention finally rules")

    resources = ROOT / "docs" / "future" / "language" / "resources-and-storage.md"
    if resources.exists():
        text = read_text(resources)
        required = [
            "map/set collection family",
            "unbuilt future surface",
            "local map/set values",
            "insert(path)",
            "set[K]",
            "map[K, V] saved-member spelling",
        ]
        for phrase in required:
            if not has_phrase(text, phrase):
                failures.append(f"{rel(resources)} missing {phrase!r}")
        shipped_patterns = [
            r"map\[K,\s*V\] saved-member sugar is shipped",
            r"map\[K,\s*V\] saved-member spelling is shipped",
            r"shipped map\[K,\s*V\] saved-member",
            r"map\[K,\s*V\].{0,120}accepted today",
        ]
        for pattern in shipped_patterns:
            if re.search(pattern, text, flags=re.IGNORECASE | re.DOTALL):
                failures.append(f"{rel(resources)} describes map[K, V] saved-member sugar as shipped")
    else:
        failures.append(f"{rel(resources)} is missing")

    data_tools = ROOT / "docs" / "future" / "data-tools.md"
    if data_tools.exists():
        text = read_text(data_tools)
        for phrase in (
            "state-vs-state",
            "equal-epoch baseline",
            "cross-epoch comparison is the growth direction",
        ):
            if not has_phrase(text, phrase):
                failures.append(f"{rel(data_tools)} missing {phrase!r}")
    else:
        failures.append("docs/future/data-tools.md is missing")

    if failures:
        print("docs_lint failed:")
        for failure in failures:
            print(f"- {failure}")
        return 1

    stale_hits = removed_surface_scan.collect_hits()
    if stale_hits:
        print("docs_lint failed:")
        print("- removed-surface scan found stale spelling:")
        for hit in stale_hits:
            print(f"  {hit.path}:{hit.line_no}: {hit.pattern_id}: {hit.text}")
        return 1

    print("docs_lint passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
