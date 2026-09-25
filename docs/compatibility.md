# Compatibility

Marrow is unreleased. This revision promises the interfaces below and nothing
beyond them.

## Versioning

The package version is `0.1.0`. `marrow --version` prints it:

```text
marrow 0.1.0
```

There is no release tag, crates.io package, signed binary, or prebuilt
distribution. A build is identified by the source revision it was built from. A
store records the format version it was provisioned with, and a build that does
not support that version refuses to open it (`store.format_version`).

The current logical-head generation is 2. Its entry-family layout requires
fresh provisioning. Generation-1 stores remain usable with their matching
older toolchain; current tools refuse them, and generation-1 tools refuse
generation-2 stores. Neither a code-only rebind nor `import` converts a store.
A version refusal preserves the engine file, head, and envelope and occurs before engine
open ([changing the program](operations/README.md#changing-the-program)).

The current store-envelope version is 1. It records whether activation completed
or a provision, rebind or explicit envelope upgrade is pending. Ordinary access
refuses both pending records and legacy version-0 envelopes with
`store.activation_required`. `marrow recover` can explicitly upgrade a version-0
envelope after validating the present exact image, supported head and engine,
and physical and logical contents. It leaves logical data and the selected head
in place and retains the store instance. This is an envelope upgrade, not a
head-layout or program-contract migration. Version-0 readers refuse the new
envelope ([recovery](operations/README.md#recovering-a-store)).

The compiler refuses public aggregate inputs and bound durable values containing
nominal integers because their transfer and stored shapes erase nominal
intervals. Recompiling such source reports `check.unsupported`
([nominal ints](language/types-and-values.md#aliases-and-nominal-ints)).

The current image generation is 1. The verifier and runner refuse generation-0
images with `image.envelope`; their erased nominal constraints cannot be recovered
from the artifact. Current tools also refuse stores whose active binding names
any other image generation with `store.format_version`, before engine open.
Recompilation and code-only rebind do not convert those stores. The refusal
preserves the engine file, head and envelope; owner-marker bookkeeping may occur.

Keep an older store with its source, identity ledger, image and matching tools
for data extraction. Current logical backup/restore admits only supported image
and head/layout generations; it does not convert older stores. New-generation
stores require fresh provisioning or a compatible logical restore. Changing an
image header or stored binding is not a conversion. The image digest domain
separates generations but does not authenticate the compiler. Historical tools
may ignore the stored image generation; use each store with its matching tools.

## Platforms

The source builds on Linux and macOS with Rust 1.89. Opening a store on disk is
narrower than the build; [install](install.md#running-against-a-store) names the
platforms.

## Stable interfaces

Three interfaces are written for machines. A diagnostic carries a dotted code
such as `check.type`; [error codes](error-codes.md) lists every code, generated
from the registry. The `marrow` command exits `0`, `1`, or `2` ([exit
codes](tools/cli.md#exit-codes)). With `--format jsonl`, `run` and `test` print
one JSON object per line with fixed field names. Every object carries `kind`. A
`run` or `test` outcome carries `outcome`; a returned value carries `data`; a
failure carries `code`; a test carries `file`, `name`, and `span`; an interrupted
invocation carries `durable`; and the `test` summary carries `selected`, `total`,
`passed`, `failed`, and `errored`.

Human-readable message text may change between revisions. Until a release policy
exists, a structured interface may also change with the implementation; the
reference records the change in the same revision.

## Unstable interfaces

The Rust crates are internal. The public interface is the `marrow` command line,
its exit codes, its JSONL records, and the dotted diagnostic codes. A program
that links a crate directly has no compatibility promise.

Raw store files are private implementation data with no public format contract. A
store is bound to one program;
[operations](operations/README.md#changing-the-program) states which program
changes it accepts. [Logical backup/restore](operations/README.md#logical-backup-and-fresh-restore)
uses a versioned bounded transfer containing the exact image, head and canonical
entry/index cells. It is not an enduring format-support promise or a schema
conversion. Evolution beyond explicit sparse scalar additions remains future
work ([status](status.md#not-yet-available)).

The durable contract records the declared names and order of the fields of
every stored struct value and enum payload. Earlier revisions recorded only
their order, and their images spell these fields without names. A store, image,
or backup from such a revision whose resources hold a stored struct, a
payload-carrying enum, an `Option`, or a `Result` therefore does not carry over:
running the unchanged program against the store reports `store.contract_changed`,
and `marrow apply` with such an image or `marrow restore` of such a backup
reports `image.table`, creating nothing. Such data stays usable with the toolchain
that wrote it; to move forward, provision a fresh store with the current
toolchain and write the entries through the program. Stores whose resources
hold only scalar fields and payloadless enums keep their contract and image
bytes. How stored struct and payload fields are identified is not yet a stable
format, so a later revision may change these contracts again.
