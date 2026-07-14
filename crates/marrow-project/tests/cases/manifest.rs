//! Manifest schema behavior: the closed `marrow.toml` schema, required explicit
//! edition, and typed rejection of everything outside it.

use marrow_project::{Edition, Manifest, ManifestErrorKind};

#[test]
fn parses_the_minimal_manifest() {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    assert_eq!(manifest.edition(), Edition::E2026);
    assert_eq!(manifest.edition().as_str(), "2026");
}

#[test]
fn edition_is_required() {
    let error = Manifest::parse("").expect_err("missing edition rejects");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ManifestErrorKind::MissingEdition);
    assert!(error.position.is_none());
}

#[test]
fn unknown_key_rejects() {
    let error =
        Manifest::parse("edition = \"2026\"\nname = \"app\"\n").expect_err("unknown key rejects");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(
        error.kind,
        ManifestErrorKind::UnknownKey {
            key: "name".to_string()
        }
    );
}

#[test]
fn unknown_key_report_is_deterministic() {
    // `toml::Table` sorts its keys, so the first unknown key reported does not
    // depend on the order the keys appear in the source.
    let error =
        Manifest::parse("zeta = 1\nedition = \"2026\"\nalpha = 2\n").expect_err("unknown keys");
    assert_eq!(
        error.kind,
        ManifestErrorKind::UnknownKey {
            key: "alpha".to_string()
        }
    );
}

#[test]
fn unsupported_edition_rejects() {
    let error = Manifest::parse("edition = \"1999\"\n").expect_err("unsupported edition rejects");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(
        error.kind,
        ManifestErrorKind::UnsupportedEdition {
            edition: "1999".to_string()
        }
    );
}

#[test]
fn non_string_edition_rejects() {
    let error = Manifest::parse("edition = 2026\n").expect_err("numeric edition rejects");
    assert_eq!(error.kind, ManifestErrorKind::EditionNotString);
}

#[test]
fn malformed_toml_carries_its_position() {
    let error = Manifest::parse("edition = \n").expect_err("malformed rejects");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ManifestErrorKind::Malformed);
    let position = error
        .position
        .expect("a located syntax fault carries a position");
    assert_eq!(position.line, 1);
    assert!(position.column >= 1);
}

#[test]
fn malformed_toml_position_tracks_later_lines() {
    let error = Manifest::parse("edition = \"2026\"\n\n= bad\n").expect_err("malformed rejects");
    let position = error
        .position
        .expect("a located syntax fault carries a position");
    assert_eq!(position.line, 3);
}

#[test]
fn duplicate_key_rejects() {
    let error = Manifest::parse("edition = \"2026\"\nedition = \"2026\"\n")
        .expect_err("duplicate key rejects");
    assert_eq!(error.code, "config.invalid");
    assert_eq!(error.kind, ManifestErrorKind::Malformed);
}

#[test]
fn arbitrary_bytes_never_panic() {
    // Parsing is total: any input yields a manifest or a typed error, never a panic.
    for input in [
        "",
        "\0",
        "[[[",
        "edition = \"2026\"",
        "edition = true",
        "[table]\nedition = \"2026\"",
        "edition = \"2026\"\r\n",
    ] {
        let _ = Manifest::parse(input);
    }
}
