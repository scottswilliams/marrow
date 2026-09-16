#!/usr/bin/env bash
# Stage an installable Marrow toolchain directory from this checkout.
#
#   scripts/stage-release.sh [--out <dir>] [--allow-dirty]
#
# Release-builds `marrow`, `marrow-runner` and `marrow-lsp` with `--locked`,
# copies them into `<out>/<short-rev>-<os>-<arch>/`, and writes the
# `marrow-companions` release manifest beside them so the store commands find
# and verify their companion runner. The staged directory is the supported
# install layout (docs/install.md): copy it onto a machine and put it on PATH.
#
# --out places the staged directory elsewhere (default: `dist/` in the
# checkout). --allow-dirty stages from a tree with uncommitted changes; without
# it a dirty tree is refused, because the recorded revision would not name the
# staged bytes.
#
# Re-running is idempotent: the staged directory is rebuilt from scratch each
# time, so a stale file from an earlier run never survives into an install.
#
# Requires `cargo`, `git` and `shasum`, and `CARGO_TARGET_DIR` set to an
# absolute path outside the checkout (CONTRIBUTING.md).

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)

usage() {
    echo "usage: scripts/stage-release.sh [--out <dir>] [--allow-dirty]" >&2
    exit 2
}

die() {
    echo "stage-release.sh: $*" >&2
    exit 1
}

OUT="$ROOT/dist"
ALLOW_DIRTY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || usage; OUT=$2; shift 2 ;;
        --allow-dirty) ALLOW_DIRTY=1; shift ;;
        *) usage ;;
    esac
done

for tool in cargo git shasum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not on PATH"
done

# The workspace rule: every cargo command names a build directory outside the
# checkout, so no lane writes ./target or shares another lane's.
[ -n "${CARGO_TARGET_DIR:-}" ] \
    || die "set CARGO_TARGET_DIR to an absolute build directory outside the checkout (CONTRIBUTING.md)"
case "$CARGO_TARGET_DIR" in
    /*) ;;
    *) die "CARGO_TARGET_DIR must be an absolute path: $CARGO_TARGET_DIR" ;;
esac

REVISION=$(git -C "$ROOT" rev-parse --verify HEAD) || die "not a git checkout: $ROOT"
SHORT_REVISION=$(git -C "$ROOT" rev-parse --short HEAD)
if [ -n "$(git -C "$ROOT" status --porcelain)" ]; then
    TREE=dirty
else
    TREE=clean
fi
if [ "$TREE" = dirty ] && [ "$ALLOW_DIRTY" -eq 0 ]; then
    die "the tree has uncommitted changes; commit them or pass --allow-dirty"
fi

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
STAGE="$OUT/$SHORT_REVISION-$OS-$ARCH"

cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" \
    -p marrow -p marrow-runner -p marrow-lsp

BUILT="$CARGO_TARGET_DIR/release"
for tool in marrow marrow-runner marrow-lsp; do
    [ -x "$BUILT/$tool" ] || die "the release build produced no $tool in $BUILT"
done

rm -rf "$STAGE"
mkdir -p "$STAGE"
for tool in marrow marrow-runner marrow-lsp; do
    cp "$BUILT/$tool" "$STAGE/$tool"
    chmod 755 "$STAGE/$tool"
done

sha256() {
    shasum -a 256 "$1" | cut -d' ' -f1
}

# The companion release identity of a runner binary:
# SHA-256( "marrow.release.companion" || u64_be(len) || bytes ), the shared
# length-delimited domain-separated construction `marrow_image::companion_release_id`
# computes. The terminal recomputes it over the staged runner before spawning it,
# so a wrong value here is refused as installation damage rather than executed.
companion_release_id() {
    local path=$1 length hex offset
    length=$(wc -c <"$path")
    length=${length//[[:space:]]/}
    hex=$(printf '%016x' "$length")
    {
        printf 'marrow.release.companion'
        for ((offset = 0; offset < 16; offset += 2)); do
            printf "\\x${hex:offset:2}"
        done
        cat "$path"
    } | shasum -a 256 | cut -d' ' -f1
}

VERSION=$("$STAGE/marrow" --version)
RELEASE=${VERSION#marrow }
RUNNER_ID=$(companion_release_id "$STAGE/marrow-runner")
printf 'marrow companions v0\nrelease %s\nrunner marrow-runner %s\nend\n' \
    "$RELEASE" "$RUNNER_ID" >"$STAGE/marrow-companions"

echo "marrow release staging"
echo "  source revision     $REVISION"
echo "  tree                $TREE"
echo "  staged directory    $STAGE"
echo "  marrow --version    $VERSION"
echo "  marrow              $(sha256 "$STAGE/marrow")"
echo "  marrow-runner       $(sha256 "$STAGE/marrow-runner")"
echo "  marrow-lsp          $(sha256 "$STAGE/marrow-lsp")"
echo "  marrow-companions   $(sha256 "$STAGE/marrow-companions")"
