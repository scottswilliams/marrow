use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy)]
pub(crate) enum Style {
    Success,
    Warning,
    Error,
    Code,
    Muted,
}

impl Style {
    fn ansi(self) -> &'static str {
        match self {
            Self::Success => "\x1b[32m",
            Self::Warning => "\x1b[33m",
            Self::Error => "\x1b[31m",
            Self::Code => "\x1b[36m",
            Self::Muted => "\x1b[2m",
        }
    }
}

pub(crate) fn paint(stream: Stream, style: Style, text: impl AsRef<str>) -> String {
    paint_if(color_enabled(stream), style, text.as_ref())
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
        assert_eq!(paint_if(true, Style::Success, "ok:"), "\x1b[32mok:\x1b[0m");
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
        assert_eq!(paint_if(false, Style::Success, "ok:"), "ok:");
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
