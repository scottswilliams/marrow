#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXPECTED_DEPENDENCIES = 28
BINARY_BASELINE_BYTES = 5_860_128
BINARY_MAX_BYTES = 7_325_160


def run(args: list[str], env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def required_output(args: list[str], env: dict[str, str] | None = None) -> str:
    result = run(args, env)
    if result.returncode != 0:
        if result.stdout:
            print(result.stdout, end="")
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)
        raise SystemExit(result.returncode)
    return result.stdout


def host_triple() -> str:
    output = required_output(["rustc", "-vV"])
    for line in output.splitlines():
        if line.startswith("host: "):
            return line.split(":", 1)[1].strip()
    raise SystemExit("rustc -vV did not report a host triple")


def cargo_env(target_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    return env


def dependency_count(manifest: Path, target: str, target_dir: Path) -> int:
    output = required_output(
        [
            "cargo",
            "tree",
            "-p",
            "marrow",
            "--locked",
            "--target",
            target,
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--manifest-path",
            str(manifest),
        ],
        cargo_env(target_dir),
    )
    seen: set[str] = set()
    root = None
    for line in output.splitlines():
        package = line.removesuffix(" (*)")
        if not package:
            continue
        if root is None:
            root = package
            continue
        if package != root:
            seen.add(package)
    return len(seen)


def build_release(manifest: Path, target: str, target_dir: Path) -> Path:
    result = run(
        [
            "cargo",
            "build",
            "--release",
            "-p",
            "marrow",
            "--locked",
            "--target",
            target,
            "--manifest-path",
            str(manifest),
        ],
        cargo_env(target_dir),
    )
    if result.stdout:
        print(result.stdout, end="")
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    if result.returncode != 0:
        raise SystemExit(result.returncode)
    return target_dir / target / "release" / "marrow"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest-path", type=Path, default=ROOT / "Cargo.toml")
    parser.add_argument("--target", default=None)
    parser.add_argument("--target-dir", type=Path, required=True)
    args = parser.parse_args()

    manifest = args.manifest_path.resolve()
    target_dir = args.target_dir.resolve()
    target = args.target or host_triple()

    rustc = required_output(["rustc", "-vV"]).strip()
    cargo = required_output(["cargo", "--version"]).strip()
    deps = dependency_count(manifest, target, target_dir)
    if deps != EXPECTED_DEPENDENCIES:
        print(f"dependency_count={deps} expected={EXPECTED_DEPENDENCIES}")
        return 1

    binary = build_release(manifest, target, target_dir)
    binary_bytes = binary.stat().st_size
    if binary_bytes > BINARY_MAX_BYTES:
        print(
            f"binary_bytes={binary_bytes} exceeds limit={BINARY_MAX_BYTES} "
            f"baseline={BINARY_BASELINE_BYTES}"
        )
        return 1

    print("release_tidy passed")
    print(f"target={target}")
    print(f"manifest={manifest}")
    print(f"target_dir={target_dir}")
    print(f"dependency_count={deps}")
    print(f"binary_bytes={binary_bytes}")
    print(f"binary_baseline_bytes={BINARY_BASELINE_BYTES}")
    print(f"binary_max_bytes={BINARY_MAX_BYTES}")
    print(f"binary_sha256={sha256(binary)}")
    print("rustc:")
    print(rustc)
    print(f"cargo: {cargo}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
