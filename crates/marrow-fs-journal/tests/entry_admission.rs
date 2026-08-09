//! Entry-name admission: every rejection class, one by one, plus the
//! admitted grammar. Admission is pure and platform-independent, so these
//! assertions hold identically on every qualified platform.

use marrow_fs_journal::{EntryName, EntryNameError};

#[test]
fn ordinary_names_are_admitted() {
    for name in [
        "store",
        "ids",
        "cache.pending",
        "a",
        "x.pending.create",
        "UPPER.lower-mixed_09",
        "name.with.dots",
        "...", // three dots is a normal name, unlike `.` and `..`
        "trailing.",
        "unicode-café",
    ] {
        let admitted = EntryName::admit(name).expect("admit an ordinary name");
        assert_eq!(admitted.as_str(), name);
        assert_eq!(admitted.to_string(), name);
    }
}

#[test]
fn the_empty_spelling_is_rejected() {
    assert_eq!(EntryName::admit(""), Err(EntryNameError::Empty));
}

#[test]
fn dot_is_rejected() {
    assert_eq!(EntryName::admit("."), Err(EntryNameError::Dot));
}

#[test]
fn dot_dot_is_rejected() {
    assert_eq!(EntryName::admit(".."), Err(EntryNameError::DotDot));
}

#[test]
fn interior_nul_is_rejected() {
    assert_eq!(EntryName::admit("a\0b"), Err(EntryNameError::Nul));
    assert_eq!(EntryName::admit("\0"), Err(EntryNameError::Nul));
}

#[test]
fn absolute_spellings_are_rejected() {
    assert_eq!(EntryName::admit("/etc"), Err(EntryNameError::Absolute));
    assert_eq!(EntryName::admit("/"), Err(EntryNameError::Absolute));
    assert_eq!(EntryName::admit("\\server"), Err(EntryNameError::Absolute));
    // A drive-prefixed spelling is absolute on the platform family that
    // reads it, so it is refused everywhere.
    assert_eq!(EntryName::admit("C:name"), Err(EntryNameError::Absolute));
    assert_eq!(EntryName::admit("c:"), Err(EntryNameError::Absolute));
}

#[test]
fn separators_are_rejected() {
    assert_eq!(EntryName::admit("a/b"), Err(EntryNameError::Separator));
    assert_eq!(EntryName::admit("a/"), Err(EntryNameError::Separator));
    assert_eq!(EntryName::admit("a\\b"), Err(EntryNameError::Separator));
}

#[test]
fn control_bytes_are_rejected() {
    assert_eq!(EntryName::admit("a\nb"), Err(EntryNameError::Control));
    assert_eq!(EntryName::admit("a\tb"), Err(EntryNameError::Control));
    assert_eq!(EntryName::admit("bell\u{7}"), Err(EntryNameError::Control));
    assert_eq!(EntryName::admit("del\u{7f}"), Err(EntryNameError::Control));
}

#[test]
fn the_name_length_bound_is_255_bytes() {
    let longest = "a".repeat(255);
    assert!(EntryName::admit(&longest).is_ok());

    let over = "a".repeat(256);
    assert_eq!(
        EntryName::admit(&over),
        Err(EntryNameError::TooLong { len: 256 })
    );

    // The bound is bytes, not characters: 128 two-byte characters fit, 128
    // plus one more do not.
    let multibyte_ok = "é".repeat(127);
    assert!(EntryName::admit(&multibyte_ok).is_ok());
    let multibyte_over = "é".repeat(128);
    assert_eq!(
        EntryName::admit(&multibyte_over),
        Err(EntryNameError::TooLong { len: 256 })
    );
}
