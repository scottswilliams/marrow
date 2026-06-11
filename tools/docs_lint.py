from pathlib import Path
import re
import sys


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
        "Nothing",
        "Default",
        "Transform",
        "Approve",
        "NoOp",
        "CatalogOnly",
        "DataProof",
        "CompatibilityLensRequired",
        "DerivedRebuild",
        "TypedTransformRequired",
        "DestructiveDecisionRequired",
        "EngineRecompileRequired",
        "RepairRequired",
    ]
    if rel(path) == "docs/implementation/check/evolution.md":
        missing = [name for name in canonical if f"`{name}`" not in text]
        if missing:
            failures.append(
                f"{rel(path)} missing canonical evolution outcome names: {', '.join(missing)}"
            )

    aliases = {
        r"\bIndexDropped\b": "use the canonical evolution outcome vocabulary",
        r"\bDefault\s*\{\s*value\s*\}": (
            "use CompatibilityLensRequired or the Default obligation"
        ),
        r"\bTransform\s*\{\s*reads\s*\}": (
            "use TypedTransformRequired or the Transform obligation"
        ),
        r"\bDestructiveDecisionRequired\s*\{": "name DestructiveDecisionRequired without payload prose",
        r"\bRepairRequired\s*\{": "name RepairRequired without payload prose",
        r"\bNo Op\b": "use NoOp when naming the internal outcome",
        r"\bIndex Rebuild\b": "use DerivedRebuild when naming the internal outcome",
        r"\bData Proof\b": "use DataProof when naming the internal outcome",
        r"\bCatalog Only\b": "use CatalogOnly when naming the internal outcome",
        r"\bCompatibility Lens Required\b": (
            "use CompatibilityLensRequired when naming the internal outcome"
        ),
        r"\bDerived Rebuild\b": "use DerivedRebuild when naming the internal outcome",
        r"\bTyped Transform Required\b": (
            "use TypedTransformRequired when naming the internal outcome"
        ),
        r"\bDestructive Decision Required\b": (
            "use DestructiveDecisionRequired when naming the internal outcome"
        ),
        r"\bEngine Recompile Required\b": (
            "use EngineRecompileRequired when naming the internal outcome"
        ),
        r"\bRepair Required\b": "use RepairRequired when naming the internal outcome",
    }
    for pattern, reason in aliases.items():
        if re.search(pattern, text):
            failures.append(f"{rel(path)} contains non-canonical outcome name {pattern!r}: {reason}")


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

    print("docs_lint passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
