//! `marrow init`: create the v0.1 quickstart project scaffold.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{CheckFormat, report_simple_error};

pub(crate) fn init_os(args: &[OsString]) -> ExitCode {
    let mut target = None;
    let mut client = false;
    for arg in args {
        match arg.to_str() {
            Some("--help" | "-h") => {
                print!(
                    "\
Usage:
  marrow init [--client] <projectdir>

Create a new Marrow project directory with the v0.1 quickstart scaffold.
The target directory must not already exist, and its final path component must
be a valid Marrow module identifier.

  --client, -c  Also scaffold a surface over the store and a generated TypeScript client.
"
                );
                return ExitCode::SUCCESS;
            }
            Some("--client" | "-c") => client = true,
            Some(value) if value.starts_with('-') => return crate::unknown_option("init", value),
            _ => {
                if let Err(code) = take_single_target(&mut target, arg, "init", "project directory")
                {
                    return code;
                }
            }
        }
    }

    let Some(target) = target else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(&target);
    let Some(name) = target_module_name(&path) else {
        report_invalid_target_name(&path);
        return ExitCode::FAILURE;
    };
    if path.exists() {
        report_simple_error(
            "config.invalid",
            "target directory already exists",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        report_simple_error(
            "config.invalid",
            &format!(
                "cannot create {} because its parent directory {} does not exist; \
                 create the parent first, or pass a target whose parent exists",
                path.display(),
                parent.display()
            ),
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }

    match write_scaffold(&path, &name, client) {
        Ok(()) => {
            println!("created {}", path.display());
            println!("next steps:");
            println!("  cd {}", path.display());
            println!("  marrow run .    # run the project and write its store and marrow.lock");
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_simple_error(
                "io.write",
                &format!("failed to create {}: {error}", path.display()),
                CheckFormat::Text,
            );
            ExitCode::FAILURE
        }
    }
}

fn take_single_target(
    slot: &mut Option<OsString>,
    target: &OsString,
    command: &str,
    target_label: &str,
) -> Result<(), ExitCode> {
    if slot.replace(target.clone()).is_some() {
        eprintln!("marrow {command} accepts one {target_label}");
        return Err(ExitCode::from(2));
    }
    Ok(())
}

fn target_module_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    valid_module_name(name).then(|| name.to_string())
}

fn valid_module_name(name: &str) -> bool {
    let parsed = marrow_syntax::parse_source(&format!("module {name}\n"));
    !parsed.has_errors()
        && parsed
            .file
            .module
            .as_ref()
            .is_some_and(|module| module.name == name && !module.name.contains("::"))
}

fn report_invalid_target_name(path: &Path) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    report_simple_error(
        "config.invalid",
        &format!(
            "project name `{name}` is not a valid Marrow module identifier: it must start with a \
             letter or underscore, then contain only letters, digits, and underscores, and may \
             not contain `::` (for example, `my_app`)"
        ),
        CheckFormat::Text,
    );
}

fn write_scaffold(target: &Path, name: &str, client: bool) -> io::Result<()> {
    fs::create_dir(target)?;
    fs::create_dir_all(target.join("src").join(name))?;
    fs::create_dir(target.join("tests"))?;
    write_new_file(target.join("marrow.json"), &config_source(name, client))?;
    write_new_file(
        target.join("src").join(name).join("books.mw"),
        &books_source(name, client),
    )?;
    write_new_file(target.join("tests/books_test.mw"), &books_test_source(name))?;
    Ok(())
}

fn write_new_file(path: PathBuf, contents: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents.as_bytes())
}

fn config_source(name: &str, client: bool) -> String {
    let client_line = if client {
        ",\n  \"client\": \"generated/marrow.ts\""
    } else {
        ""
    };
    format!(
        r#"{{
  "sourceRoots": ["src"],
  "run": {{ "defaultEntry": "{name}::books::main" }},
  "store": {{ "backend": "native", "dataDir": ".marrow/data" }},
  "tests": ["tests"]{client_line}
}}
"#
    )
}

fn books_source(name: &str, client: bool) -> String {
    let surface = if client {
        "\nsurface Books from ^books\n    fields title, author, shelf\n    collection ^books.byShelf as byShelf\n"
    } else {
        ""
    };
    format!(
        r#"module {name}::books

resource Book
    required title: string
    required author: string
    required shelf: string
    loanedTo: string

store ^books(id: int): Book
    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    const id: Id(^books) = nextId(^books)
    ^books(id) = book
    return id

pub fn listShelf(shelf: string)
    for id, book in ^books.byShelf(shelf)
        print($"{{id}}: {{book.title}} by {{book.author}}")

pub fn main()
    add(title: "Small Gods", author: "Terry Pratchett", shelf: "fiction")
    add(title: "Sourcery", author: "Terry Pratchett", shelf: "fiction")
    listShelf("fiction")
{surface}"#
    )
}

fn books_test_source(name: &str) -> String {
    format!(
        r#"module tests::books_test

use {name}::books

pub fn addThenFind()
    const id = books::add(title: "Mort", author: "Terry Pratchett", shelf: "fiction")
    std::assert::isTrue(exists(^books(id)))
    if const title = ^books(id).title
        std::assert::isTrue(title == "Mort")
    else
        std::assert::isTrue(false)
"#
    )
}
