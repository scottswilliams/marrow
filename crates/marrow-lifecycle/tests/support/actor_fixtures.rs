//! Compiled source fixtures shared by lifecycle admission tests.

/// A durable program exercising every split-order decision point across more than one shape:
/// **two roots** (`books`, `tags` — the outer declaration-order loop), a resource with **two
/// top-level fields** (field order), **two sibling groups** each of one field (group order and
/// the group-then-its-members split), and a **nested branch** (`notes` carrying a `replies`
/// sub-branch — the recursive branch descent). A single-shape fixture would leave the ordering
/// and recursion split — the only place the kernel and head-map walks could diverge —
/// under-driven.
pub const GRAPH_SOURCE: &str = r#"resource Tag {
    required name: string
}

resource Book {
    required title: string
    subtitle: string

    details {
        pages: int
    }

    meta {
        isbn: string
    }

    notes[noteId: string] {
        required body: string

        replies[replyId: string] {
            required text: string
        }
    }
}

store ^books[id: int]: Book
store ^tags[id: int]: Tag

pub fn readTitle(id: int): string {
    return ^books[id].title ?? "?"
}
"#;

/// The identity ledger for [`GRAPH_SOURCE`]: every durable anchor — two products, every field,
/// the two groups, the `notes` branch and its nested `replies` sub-branch (each a `root`-
/// anchored placement, keys and fields path-qualified through the branch chain), and both
/// store roots with their keys.
pub const GRAPH_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.subtitle 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id group Book.meta 22222222222222222222222222222222\n\
     id field Book.meta.isbn 23232323232323232323232323232323\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.body 32323232323232323232323232323232\n\
     id root Book.notes.replies 33333333333333333333333333333333\n\
     id key Book.notes.replies.replyId 34343434343434343434343434343434\n\
     id field Book.notes.replies.text 35353535353535353535353535353535\n\
     id product Tag 40404040404040404040404040404040\n\
     id field Tag.name 41414141414141414141414141414141\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root tags 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     id key tags.id 4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c\n\
     high-water 0\n\
     end\n";

/// Compile a project of several modules, so an export's *module* — half of its declaration
/// path identity — can be varied as well as its item name.
pub fn compile_files(sources: &[(&str, &str)], ids: &str) -> Vec<u8> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = sources
        .iter()
        .map(|(path, text)| {
            marrow_project::CapturedFile::new(path.to_string(), text.as_bytes().to_vec())
        })
        .collect();
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    compiled.image.bytes
}
