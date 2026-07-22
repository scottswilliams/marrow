//! `marrow image`: the deployment image-emit command and its accepted-ceiling gate.
//!
//! A project travels the real production path through the built binary — capture,
//! compile, independent verify — and the verified `program.image` is written only
//! when the owner accepts the image's own deployment ceiling id. The command is the
//! stock way a `MarrowDeployment` composes its verified image; these thin boundary
//! tests pin the acceptance gate (no image on a missing or wrong ceiling id) and the
//! byte-deterministic emission, which are the enforcement artifacts for
//! "deployment composition names the accepted ceiling; no target-runtime widening".

mod common;

use common::Project;

/// The Workshop-shaped two-root fixture inline: one read, one cross-root write, so the
/// image's demand union is nonempty and its ceiling id is stable.
const SOURCE: &str = "\
resource Tally {\n\
\x20   required count: int\n\
}\n\
\n\
store ^tallies[name: string]: Tally\n\
\n\
pub fn bump(name: string) {\n\
\x20   transaction {\n\
\x20       const prior = ^tallies[name].count ?? 0\n\
\x20       ^tallies[name].count = prior + 1\n\
\x20   }\n\
}\n\
\n\
pub fn peek(name: string): int {\n\
\x20   return ^tallies[name].count ?? 0\n\
}\n";

/// The minted identity ledger for the fixture, so the CLI compile path (which never
/// mints) has stable durable ids.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Tally 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id field Tally.count 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id root tallies 4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a\n\
     id key tallies.name 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     high-water 0\n\
     end\n";

fn project() -> Project {
    Project::single(SOURCE).ids(IDS)
}

/// Parse the `ceiling <id>` line the command prints on standard error when the
/// acceptance argument is absent.
fn ceiling_id_from_unaccepted(stderr: &str) -> String {
    let marker = "deployment ceiling id is ";
    let start = stderr.find(marker).expect("stderr names the ceiling id") + marker.len();
    let rest = &stderr[start..];
    let end = rest.find(';').expect("ceiling id is delimited");
    rest[..end].trim().to_string()
}

/// Without `--accept-ceiling` the command writes no image, exits nonzero with
/// `cli.ceiling_unaccepted`, and prints the image's ceiling id and its per-export
/// demand for the owner to review.
#[test]
fn image_requires_the_owner_to_accept_the_ceiling() {
    let workspace = project().materialize("image-unaccepted");
    let outcome = workspace.marrow(&["image", "--out", "deploy"]);

    assert!(!outcome.success(), "must refuse without acceptance");
    let stderr = outcome.stderr_text();
    assert!(
        stderr.contains("cli.ceiling_unaccepted"),
        "typed code on stderr: {stderr}"
    );
    // The rendered demand names the durable places the ceiling would admit.
    assert!(
        stderr.contains("writes ^tallies") && stderr.contains("reads ^tallies"),
        "demand is rendered for review: {stderr}"
    );
    assert!(
        !workspace.path("deploy/program.image").exists(),
        "no image is written when the ceiling is unaccepted"
    );
}

/// Accepting the exact ceiling id the command prints writes the verified image and
/// reports the image id and accepted ceiling id on standard output.
#[test]
fn image_writes_the_verified_image_on_accepting_the_ceiling() {
    let workspace = project().materialize("image-accepted");
    let ceiling =
        ceiling_id_from_unaccepted(&workspace.marrow(&["image", "--out", "d0"]).stderr_text());

    let outcome = workspace.marrow(&["image", "--out", "deploy", "--accept-ceiling", &ceiling]);
    assert!(
        outcome.success(),
        "accepting the ceiling composes the image: {}",
        outcome.stderr_text()
    );

    let stdout = outcome.stdout_text();
    assert!(
        stdout.contains(&format!("ceiling {ceiling}")),
        "stdout pins the ceiling id: {stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.starts_with("image ")),
        "stdout pins the image id: {stdout}"
    );
    assert!(
        workspace.path("deploy/program.image").exists(),
        "the verified image is written"
    );
}

/// A ceiling id that does not match the image writes nothing and fails closed: an
/// owner cannot widen or narrow the deployment's durable authority by accident.
#[test]
fn a_wrong_ceiling_id_writes_no_image() {
    let workspace = project().materialize("image-mismatch");
    let outcome = workspace.marrow(&[
        "image",
        "--out",
        "deploy",
        "--accept-ceiling",
        "00000000000000000000000000000000000000000000000000000000deadbeef",
    ]);
    assert!(!outcome.success(), "a wrong ceiling id must be refused");
    assert!(
        outcome.stderr_text().contains("cli.ceiling_unaccepted"),
        "typed code on a mismatch: {}",
        outcome.stderr_text()
    );
    assert!(
        !workspace.path("deploy/program.image").exists(),
        "no image is written on a mismatch"
    );
}

/// Emission is byte-deterministic: two accepted emissions of the same source produce
/// the identical image file, so a deployment build is reproducible.
#[test]
fn image_emission_is_byte_deterministic() {
    let workspace = project().materialize("image-deterministic");
    let ceiling =
        ceiling_id_from_unaccepted(&workspace.marrow(&["image", "--out", "d0"]).stderr_text());

    assert!(
        workspace
            .marrow(&["image", "--out", "a", "--accept-ceiling", &ceiling])
            .success()
    );
    assert!(
        workspace
            .marrow(&["image", "--out", "b", "--accept-ceiling", &ceiling])
            .success()
    );

    let a = std::fs::read(workspace.path("a/program.image")).expect("read a");
    let b = std::fs::read(workspace.path("b/program.image")).expect("read b");
    assert_eq!(a, b, "the emitted image is byte-identical across runs");
}
