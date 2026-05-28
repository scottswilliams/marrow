use std::process::ExitCode;

use serde_json::json;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--format text|json|jsonl] <file.mw>
  marrow fmt [--check | --write] <file.mw>
  marrow --version
  marrow --help

Marrow is starting from the reference docs. Language commands will land as the
native .mw source model, checker, runtime, and storage kernel grow.
";

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "check") {
        return check(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "fmt") {
        return fmt(&args[1..]);
    }
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None | Some("--help" | "-h" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V" | "version") => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command: {command}");
            eprintln!("run `marrow --help` for available commands");
            ExitCode::from(2)
        }
    }
}

fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return ExitCode::from(2);
                };
                let Some(parsed) = CheckFormat::parse(value) else {
                    eprintln!("unknown check format: {value}");
                    return ExitCode::from(2);
                };
                format = parsed;
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow check [--format text|json|jsonl] <file.mw>

Parse a Marrow source file and report syntax diagnostics.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown check option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if file.replace(value.to_string()).is_some() {
                    eprintln!("marrow check accepts one source file");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(file) = file else {
        eprintln!("missing source file");
        return ExitCode::from(2);
    };
    let source = match std::fs::read_to_string(&file) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&file, &error, format);
            return ExitCode::FAILURE;
        }
    };
    let parsed = marrow_syntax::parse_source(&source);
    report_check(&file, &parsed, format);
    if parsed.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn fmt(args: &[String]) -> ExitCode {
    let mut mode = FmtMode::Print;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => mode = FmtMode::Check,
            "--write" => mode = FmtMode::Write,
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow fmt [--check | --write] <file.mw>

Format a Marrow source file. With no flag, print the formatted source to
stdout. --check exits non-zero if the file is not already formatted. --write
rewrites the file in place.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown fmt option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if file.replace(value.to_string()).is_some() {
                    eprintln!("marrow fmt accepts one source file");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(file) = file else {
        eprintln!("missing source file");
        return ExitCode::from(2);
    };
    let source = match std::fs::read_to_string(&file) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&file, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };

    // Do not reformat source that does not parse; report its diagnostics and
    // leave the file untouched.
    let parsed = marrow_syntax::parse_source(&source);
    if parsed.has_errors() {
        report_check(&file, &parsed, CheckFormat::Text);
        return ExitCode::FAILURE;
    }

    let formatted = marrow_syntax::format_source(&source);
    match mode {
        FmtMode::Print => {
            print!("{formatted}");
            ExitCode::SUCCESS
        }
        FmtMode::Check => {
            if source == formatted {
                ExitCode::SUCCESS
            } else {
                eprintln!("{file}: not formatted");
                ExitCode::FAILURE
            }
        }
        FmtMode::Write => {
            if source != formatted
                && let Err(error) = std::fs::write(&file, &formatted)
            {
                report_io_error(&file, &error, CheckFormat::Text);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
    }
}

#[derive(Clone, Copy)]
enum FmtMode {
    Print,
    Check,
    Write,
}

#[derive(Clone, Copy)]
enum CheckFormat {
    Text,
    Json,
    Jsonl,
}

impl CheckFormat {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

fn report_check(file: &str, parsed: &marrow_syntax::ParsedSource, format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            if parsed.diagnostics.is_empty() {
                println!(
                    "ok: {file} parsed ({} declaration{})",
                    parsed.file.declarations.len(),
                    if parsed.file.declarations.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            } else {
                for diagnostic in &parsed.diagnostics {
                    eprintln!("{file}:{diagnostic}");
                    if let Some(help) = &diagnostic.help {
                        eprintln!("help: {help}");
                    }
                }
            }
        }
        CheckFormat::Json => {
            let diagnostics = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic_record(file, diagnostic))
                .collect::<Vec<_>>();
            write_json(json!({
                "file": file,
                "status": if parsed.has_errors() { "failed" } else { "ok" },
                "diagnostics": diagnostics,
                "declarations": parsed.file.declarations.len(),
            }));
        }
        CheckFormat::Jsonl => {
            for diagnostic in &parsed.diagnostics {
                write_json(diagnostic_record(file, diagnostic));
            }
            write_json(json!({
                "kind": "summary",
                "file": file,
                "status": if parsed.has_errors() { "failed" } else { "ok" },
                "diagnostics": parsed.diagnostics.len(),
                "declarations": parsed.file.declarations.len(),
            }));
        }
    }
}

fn report_io_error(file: &str, error: &std::io::Error, format: CheckFormat) {
    match format {
        CheckFormat::Text => eprintln!("io.read: failed to read {file}: {error}"),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "code": "io.read",
                "kind": "io",
                "message": format!("failed to read {file}: {error}"),
                "source_span": null,
            }));
        }
    }
}

fn diagnostic_record(file: &str, diagnostic: &marrow_syntax::Diagnostic) -> serde_json::Value {
    json!({
        "code": diagnostic.code,
        "kind": diagnostic.kind,
        "severity": diagnostic.severity.as_str(),
        "message": diagnostic.message,
        "help": diagnostic.help,
        "source_span": {
            "file": file,
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
            "start_byte": diagnostic.span.start_byte,
            "end_byte": diagnostic.span.end_byte,
        },
    })
}

fn write_json(value: serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string(&value).expect("JSON value should serialize")
    );
}
