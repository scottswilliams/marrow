# Install from source

Marrow builds from a source checkout. There is no tagged release, crates.io
package, or prebuilt binary yet; build the revision that carries the
documentation you are reading.

## Requirements

Rust 1.89 and Git, on Linux or macOS. Other operating systems are rejected at
build time. [Running against a store](#running-against-a-store) states the
additional requirements for a store on disk.

## Install

[`scripts/stage-release.sh`](../scripts/stage-release.sh) builds the toolchain
and stages a complete install directory:

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
CARGO_TARGET_DIR=/absolute/path/to/marrow-target ./scripts/stage-release.sh
```

The staged directory is `dist/<short-revision>-<os>-<arch>/` and holds four
files:

```text
marrow              the command-line tool
marrow-runner       the companion runner the store commands spawn
marrow-lsp          the editor language server
marrow-companions   the release manifest naming the runner's release identity
```

The script prints `marrow --version`, the source revision, whether the tree was
clean, and the SHA-256 of all four files. It refuses a tree with uncommitted
changes unless given `--allow-dirty`, and requires `CARGO_TARGET_DIR`
([contributing](../CONTRIBUTING.md)). Re-running it rebuilds the directory from
scratch.

Install by moving that directory where it will stay and putting it on `PATH`:

```sh
mv dist/<short-revision>-<os>-<arch> ~/.local/marrow
export PATH="$HOME/.local/marrow:$PATH"
```

Install the directory, not the individual files. `marrow` locates its companion
runner beside the path it was launched from, so a `marrow` symlinked or copied
into a directory on its own leaves the store commands reporting
`cli.installation_damaged`.

Installing starts no service and creates no data directory. The four files are
unsigned, so a copy transferred through a browser download is quarantined on
macOS; transfer the directory with `tar`, `rsync` or `scp`.

## Verify

```sh
marrow --version
```

```text
marrow 0.1.0
```

`marrow-lsp --help` prints the language server's usage. The
[quickstart](quickstart.md) starts from here.

## Running against a store

A source install runs every storeless command, and `marrow test` runs durable
tests against a fresh in-memory store. A store on disk also needs a supported
platform and the companion layout.

Opening a store on disk works on macOS, and on Linux on x86_64 and aarch64.
On other Linux architectures the toolchain builds, but opening a store stops
with a message naming the operating system and architecture.

A store on disk is opened by a companion runner. `marrow run --store` and
`marrow import` need the `marrow-runner` binary and the `marrow-companions`
manifest in the same directory as `marrow`; a staged directory has both. The
terminal verifies the runner against the manifest before spawning it, so a
missing, mismatched, or altered component stops with
`cli.installation_damaged` rather than running. `cargo install --path
crates/marrow` installs the terminal alone and does not produce that layout.
