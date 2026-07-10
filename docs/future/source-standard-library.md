# Source-defined standard library

This page is future direction. The current standard library is implemented by
Rust descriptor and runtime tables.

## Goal

Most portable library behavior should be ordinary Marrow source. Rust should
provide a small versioned intrinsic and host-import boundary for operations
that cannot be expressed portably, such as primitive arithmetic, clock and
filesystem access, cryptography, and durable-path execution.

The split should be visible:

```text
Rust bootstrap and trusted runtime
    -> minimal intrinsics and host imports
        -> versioned Marrow standard-library modules
            -> application and bundled Marrow tools
```

The standard library is the first substantial part of Marrow that should be
written in Marrow. Test assertions, image inspection, compatibility-report
rendering, and generated TypeScript binding rendering are later candidates for
bundled storeless tool images.

## Constraints

- Bundled modules are checked, compiled, verified, bounded, and invoked through
  the same machinery as application code.
- Source and canonical tool images ship with reproducible manifests and
  digests.
- Compiler facts remain owned by the compiler; Marrow tools may render them but
  may not rederive types, paths, effects, or compatibility verdicts.
- The core compiler can build and diagnose a project when optional bundled
  tools are unavailable.
- Project code is never loaded as an ambient compiler plugin.
- No standard-library feature survives merely because the current Rust runtime
  happens to implement it.

Compiler self-hosting is not a v1 requirement. A post-v1 shadow implementation
of the schema-path/effect middle end would be more relevant than self-hosting a
lexer, but it must retain a Rust seed compiler and independent verifier.
