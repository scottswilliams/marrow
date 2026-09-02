# Install from source

Marrow builds from a source checkout. There is no tagged release, crates.io
package, or prebuilt binary yet; build the revision that carries the
documentation you are reading.

## Requirements

Rust 1.89 and Git, on Linux or macOS. A build for another target runs the
storeless commands; [Running against a store](#running-against-a-store) states
what a store on disk needs.

## Install

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
cargo install --locked --path crates/marrow
cargo install --locked --path crates/marrow-lsp
```

These install `marrow`, the command-line tool, and `marrow-lsp`, the editor
language server. Installing them starts no service and creates no data
directory.

To build without installing:

```sh
cargo build --release --locked -p marrow -p marrow-lsp
./target/release/marrow --version
```

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

Opening a store on disk works on macOS, and on Linux on x86_64 and aarch64. A
build for another target runs storeless commands and `marrow test`; an attempt
to open a store stops with a message naming the operating system and
architecture.

A store on disk is opened by a companion runner. `marrow run --store` and
`marrow import` need the `marrow-runner` binary and the `marrow-companions`
manifest in the same directory as `marrow`. The two `cargo install` commands
above install `marrow` and `marrow-lsp` only; without the companion layout,
the store commands stop with `cli.installation_damaged`. A command that
installs the layout is future work ([status](status.md)).
