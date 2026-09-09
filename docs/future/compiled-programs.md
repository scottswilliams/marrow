# Compiled programs

Marrow compiles a project to an immutable program image, verifies it
independently and runs it on a bytecode VM.

## Today

Compilation opens no store or network. The image contains concrete types,
functions, exports, source maps and a durable contract. The compiler emits
bytes; only the verifier creates the verified artifact consumed by the VM.
The [implementation map](../implementation/README.md) describes these owners,
and [execution limits](../language/execution-limits.md) lists current ceilings.

These boundaries do not establish that every accepted image is correct.
Adversarial verification and the resource cost of simultaneous compiler,
verifier and runtime populations remain qualification obligations.

## Beta direction

Keep the parser, storeless compiler, image owner, independent verifier, VM,
typed path kernel and private engine boundaries. Remove duplicated allocation,
classification and source analysis within those boundaries. Add an intermediate
representation or analysis pass only for a current invariant that existing
facts cannot express. Independent verification must reconstruct its facts from
the image; it does not trust compiler state.

Values crossing maintained invocation and storage boundaries must retain the
constraints their declared types require. The beta keeps nominal-bearing
aggregate input and durable-value refusal rather than broadening their ABI.
A deliberate incompatible admission boundary must exclude older artifacts
whose erased constraints cannot be recovered. Recompilation and explicit store
format refusal are preferable to a decoder that guesses old meaning. Preserve
old stores and matching tools until complete logical extraction is verified.

## Image encoding version

The current format and limits are implementation facts, not a stable ABI.
Choose a new format only when a correctness boundary or a measured admitted
program requires it. No counter width, digest replacement, encoding succession
or wide-image project is prescribed in advance.

Native code generation, JIT compilation, compiler self-hosting and a stable
binary package ABI are outside the beta. Raw Rust embedding APIs remain
trusted-caller interfaces; a checked embedding API needs a real caller
and construction-time proof, not a second recursive scan on every invocation.

## Evidence

Pin valid and hostile source/image workloads, outputs, peak-resource bounds and
all three clocks before implementation. Test wrong-image handles, old artifacts,
malformed graphs, branch and loop amplification, and refused source with
independent diagnostics. Unsupported input must fail within the declared
envelope. A format change must refuse old material without mutating its store.
