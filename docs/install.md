# Installing Marrow

Marrow is unreleased. Install from source.

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

## Data Directories

Marrow uses explicit project configuration for persistent data. Installing or
running Marrow does not start a background service, modify the shell profile,
or create hidden data directories.

The project file is `marrow.json`. Its `store` field selects the storage
backend and data directory for commands that need saved data; there are no
command-line storage overrides.

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
same backend contract described in [`backend-contract.md`](backend-contract.md).
They are not part of the default install path.
