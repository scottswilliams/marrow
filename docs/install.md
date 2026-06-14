# Installing Marrow

Marrow v0.1.0 is distributed as a tagged source release. Install from source.

## Supported Platforms

Marrow v0.1 supports Unix targets only: Linux and macOS. Non-Unix builds are
outside the v0.1 contract; the stable-id entropy backstop may panic there rather
than report a Marrow diagnostic.

## From Source

Requirements:

- Rust stable 1.89 or newer;
- Git.

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
git checkout v0.1.0
cargo install --locked --path crates/marrow
marrow --version
```

The installed command is `marrow`. The v0.1.0 version output includes the
current engine profile:

```console
$ marrow --version
marrow 0.1.0 engine-profile=(key=v0, layout-epoch=0, digest=77944eb86c08b665)
```

## Build Without Installing

```sh
cargo build --release -p marrow --locked
./target/release/marrow --version
```

Prebuilt binaries and crates.io publication are post-v0.1 fast-follow channels,
not v0.1 release channels.

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
