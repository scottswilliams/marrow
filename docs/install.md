# Install From Source

Marrow is experimental, unreleased software. The repository does not currently
publish a tagged release, a crates.io package, or prebuilt binaries. Build the
revision that contains the documentation you are reading.

## Requirements

- Linux or macOS;
- Rust 1.89;
- Git.

Other operating systems are not supported by the current source build. On a
platform without an approved operating-system entropy source, operations that
need to allocate durable identities return an unsupported-I/O error; they do not
provide a fallback identity source.

## Install The Command

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
cargo install --locked --path crates/marrow
marrow --version
```

The package version printed by `marrow --version` describes the current binary;
it does not identify a released compatibility contract. The same output includes
the storage engine profile used to reject incompatible durable data.

To build without installing:

```sh
cargo build --release --locked --manifest-path crates/marrow/Cargo.toml
./target/release/marrow --version
```

## Project Data

Installing Marrow does not start a service, alter the shell profile, or create a
data directory. A project selects its source roots and storage in `marrow.json`.
The native store creates files only when a write-capable project command needs
them.

Continue with the [Quickstart](quickstart.md). See the
[`marrow.json` reference](tools/project-file.md) for project paths and
[Native Store Operations](operations/native-store.md) for storage ownership and
filesystem assumptions.
