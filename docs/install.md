# Installing Marrow

Marrow is unreleased. During development, install from source.

These notes describe the release shape Marrow is converging on. Source builds
may expose unfinished development commands while the implementation catches up.

## From Source

Requirements:

- Rust stable 1.89 or newer;
- Git.

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
cargo install --path crates/marrow
marrow --version
```

The installed command is `marrow`.

## Build Without Installing

```sh
cargo build --release -p marrow
./target/release/marrow --version
```

On Windows, the binary is `target\release\marrow.exe`.

## Release Package Shape

Release archives use this shape:

```text
marrow-<version>-<platform>.zip
marrow-<version>-<platform>.zip.sha256
```

The archive contains the `marrow` binary, version metadata, checksum metadata,
and short install notes. It does not install a background service, modify the
user's shell profile, or create hidden data directories.

## Data Directories

Marrow uses explicit project or command configuration for persistent data.
Hidden process-wide storage is not part of the normal application model.

The project file is `marrow.json`. It can select a store and data
directory for commands that need saved data. Command flags can override or
provide storage for one run.

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::sample::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" }
}
```

## Storage Engines

The native store is the default persistent project store and the only storage
engine required for the first release.

Other storage engines can exist as separate packages when they implement the
same backend contract described in [`implementation.md`](implementation.md).
They are not part of the default install path.
