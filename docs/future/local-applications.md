# Local applications

This page is future direction. Marrow does not currently ship a supported
desktop host or the target callable binding.

## Goal

A local application should combine a compiled Marrow program, one durable
store, and a trusted host process. The renderer receives generated typed
proxies for explicitly exported functions. It does not receive filesystem
paths, store handles, raw durable addresses, host capabilities, or maintenance
authority.

A persistent sidecar owned by the desktop main process is the likely first
host. Framed local IPC avoids introducing localhost HTTP and native-addon ABI
requirements before they are useful.

## Developer loop

The intended loop is short and explicit:

```text
edit -> check -> test -> build -> admit -> activate -> run
                         |                    |
                         +-> inspect/backup <-+
```

Development convenience must not silently bypass store admission or destructive
activation decisions.

## Evidence target

One populated application should retain real state across code-only changes,
compatible schema additions, explicit transforms, a destructive decision,
backup, restore, and recovery. Its business logic and durable model should later
run in the served profile without being rewritten around transport.

The application is a product test, not a reason to add a UI framework, generic
record editor, arbitrary schema builder, or search engine to the language.
