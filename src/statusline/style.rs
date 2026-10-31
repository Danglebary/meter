//! Lines of colored spans, and their rendering in ANSI escape codes.

/// The escape code that returns the terminal to its default color.
const RESET: &str = "\u{1b}[0m";

/// A span's color; `Plain` takes the terminal's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Plain,
    Blue,
    Green,
    Yellow,
    Red,
}

impl Color {
    const fn code(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Blue => Some("\u{1b}[0;34m"),
            Self::Green => Some("\u{1b}[0;32m"),
            Self::Yellow => Some("\u{1b}[0;33m"),
            Self::Red => Some("\u{1b}[0;31m"),
        }
    }
}

/// A run of text in one color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Color,
}

impl Span {
    #[must_use]
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

/// One line of the status line, as the spans it prints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    /// The line's text with no color.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// Whether the line holds any text to print.
    #[must_use]
    pub fn has_text(&self) -> bool {
        self.spans.iter().any(|span| !span.text.is_empty())
    }

    pub fn push(&mut self, text: impl Into<String>, color: Color) {
        self.spans.push(Span::new(text, color));
    }

    /// Pushes `separator` before the text where the line already holds some.
    pub fn push_after(&mut self, separator: &str, text: impl Into<String>, color: Color) {
        if self.has_text() {
            self.push(separator, Color::Plain);
        }
        self.push(text, color);
    }

    /// Appends another line's spans, after `separator` where both hold text.
    pub fn append(&mut self, separator: &str, other: Self) {
        if other.has_text() {
            if self.has_text() {
                self.push(separator, Color::Plain);
            }
            self.spans.extend(other.spans);
        }
    }
}

/// The lines as the terminal prints them: each colored span wrapped in its escape code and
/// a reset, and each line ended by a newline.
#[must_use]
pub fn to_ansi(lines: &[Line]) -> String {
    let mut printed = String::new();
    for line in lines {
        for span in &line.spans {
            match span.color.code() {
                Some(code) => {
                    printed.push_str(code);
                    printed.push_str(&span.text);
                    printed.push_str(RESET);
                }
                None => printed.push_str(&span.text),
            }
        }
        printed.push('\n');
    }
    printed
}
