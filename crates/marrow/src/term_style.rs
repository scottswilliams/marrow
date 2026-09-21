use marrow_codes::Code;

use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy)]
pub(crate) enum Style {
    Warning,
    Error,
    Code,
    Muted,
}

impl Style {
    fn ansi(self) -> &'static str {
        match self {
            Self::Warning => "\x1b[33m",
            Self::Error => "\x1b[31m",
            Self::Code => "\x1b[36m",
            Self::Muted => "\x1b[2m",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    enabled: bool,
}

impl Palette {
    pub(crate) fn for_stream(stream: Stream) -> Self {
        Self {
            enabled: color_enabled(stream),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(enabled: bool) -> Self {
        Self { enabled }
    }

    pub(crate) fn paint(self, style: Style, text: impl AsRef<str>) -> String {
        paint_if(self.enabled, style, text.as_ref())
    }

    pub(crate) fn code_message(self, code: Code, message: impl std::fmt::Display) -> String {
        format!("{}: {message}", self.paint(Style::Code, code.as_str()))
    }

    /// One source diagnostic as every command prints it: `file:line:column: code:
    /// message`, the file muted and the code styled. `file` is the compiler's own
    /// spelling, so a file a dependency declares carries that dependency's alias.
    pub(crate) fn diagnostic(
        self,
        file: &str,
        line: u32,
        column: u32,
        code: &str,
        message: &str,
    ) -> String {
        format!(
            "{}:{line}:{column}: {}: {message}",
            self.paint(Style::Muted, file),
            self.paint(Style::Code, code),
        )
    }
}

pub(crate) fn paint(stream: Stream, style: Style, text: impl AsRef<str>) -> String {
    Palette::for_stream(stream).paint(style, text)
}

pub(crate) fn code_message(stream: Stream, code: Code, message: impl std::fmt::Display) -> String {
    Palette::for_stream(stream).code_message(code, message)
}

pub(crate) fn render_help(stream: Stream, text: &str) -> String {
    render_help_if(color_enabled(stream), text)
}

fn color_enabled(stream: Stream) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    match stream {
        Stream::Stdout => std::io::stdout().is_terminal(),
        Stream::Stderr => std::io::stderr().is_terminal(),
    }
}

fn paint_if(enabled: bool, style: Style, text: &str) -> String {
    if enabled {
        format!("{}{text}\x1b[0m", style.ansi())
    } else {
        text.to_string()
    }
}

fn render_help_if(enabled: bool, text: &str) -> String {
    let mut rendered = String::with_capacity(text.len());
    for part in text.split_inclusive('\n') {
        let (line, newline) = part
            .strip_suffix('\n')
            .map_or((part, ""), |line| (line, "\n"));
        let line = match line {
            "Marrow" => paint_if(enabled, Style::Code, line),
            "Usage:" => paint_if(enabled, Style::Warning, line),
            _ => line.to_string(),
        };
        rendered.push_str(&line);
        rendered.push_str(newline);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_enabled_wraps_text_in_ansi_style() {
        assert_eq!(
            paint_if(true, Style::Warning, "warning:"),
            "\x1b[33mwarning:\x1b[0m"
        );
        assert_eq!(
            paint_if(true, Style::Error, "error:"),
            "\x1b[31merror:\x1b[0m"
        );
        assert_eq!(
            paint_if(true, Style::Code, "check.type"),
            "\x1b[36mcheck.type\x1b[0m"
        );
        assert_eq!(paint_if(true, Style::Muted, "path"), "\x1b[2mpath\x1b[0m");
    }

    #[test]
    fn paint_disabled_returns_plain_text() {
        assert_eq!(paint_if(false, Style::Error, "error:"), "error:");
    }

    #[test]
    fn code_message_styles_only_the_code_token() {
        assert_eq!(
            Palette::for_test(true).code_message(Code::IoWrite, "failed to write output"),
            "\x1b[36mio.write\x1b[0m: failed to write output"
        );
        assert_eq!(
            Palette::for_test(false).code_message(Code::IoWrite, "failed to write output"),
            "io.write: failed to write output"
        );
    }

    #[test]
    fn diagnostic_line_styles_the_file_and_the_code() {
        assert_eq!(
            Palette::for_test(false).diagnostic("src/a.mw", 3, 5, "check.type", "found int"),
            "src/a.mw:3:5: check.type: found int"
        );
        assert_eq!(
            Palette::for_test(true).diagnostic("lib:src/a.mw", 3, 5, "check.type", "found int"),
            "\x1b[2mlib:src/a.mw\x1b[0m:3:5: \x1b[36mcheck.type\x1b[0m: found int"
        );
    }

    #[test]
    fn help_text_keeps_plain_shape_when_color_is_disabled() {
        let help = render_help_if(false, "Marrow\n\nUsage:\n  marrow --help\n");
        assert_eq!(help, "Marrow\n\nUsage:\n  marrow --help\n");
    }

    #[test]
    fn help_text_styles_heading_and_usage_when_color_is_enabled() {
        let help = render_help_if(true, "Marrow\n\nUsage:\n  marrow --help\n");
        assert!(help.contains("\x1b[36mMarrow\x1b[0m"));
        assert!(help.contains("\x1b[33mUsage:\x1b[0m"));
        assert!(help.ends_with('\n'));
    }
}
