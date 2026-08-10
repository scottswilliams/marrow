# Install From Source

Marrow is experimental, unreleased software. The repository does not currently
publish a tagged release, a crates.io package, or prebuilt binaries. Build the
revision that contains the documentation you are reading.

## Requirements

- Linux or macOS;
- Rust 1.89;
- Git.

Other operating systems are not supported by the current source build.

Opening a persistent store is narrower than the build. A store open performs
descriptor-rooted filesystem operations that the current source provides on
macOS, and on Linux for the `x86_64` and `aarch64` architectures. A build for
any other target still compiles and still runs storeless commands; an attempt to
open a store refuses at run time, naming the operating system and architecture
it refused on.

## Install The Command

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
cargo install --locked --path crates/marrow
marrow --version
```

The package version printed by `marrow --version` describes the current binary;
it does not identify a released compatibility contract.

To build without installing:

```sh
cargo build --release --locked --manifest-path crates/marrow/Cargo.toml
./target/release/marrow --version
```

## After Installing

Installing Marrow does not start a service, alter the shell profile, or create a
data directory. Continue with the [CLI reference](tools/cli.md); the project
and durable-data workflows return with their refounding lanes — see
[Project status](status.md).
