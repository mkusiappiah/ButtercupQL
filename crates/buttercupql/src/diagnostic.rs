//! Problems found in ButtercupQL source text, and how they are shown.

use std::fmt;

use crate::span::Span;

/// How serious a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The text can't be compiled.
    Error,
    /// The text compiles, but may not do what was meant.
    Warning,
}

impl Severity {
    /// `"error"` or `"warning"`.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// Another place a problem concerns, with a word about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The place.
    pub span: Span,
    /// What it has to do with the problem: "the bracket opens here".
    pub message: String,
}

/// A problem found in a text: what it is, where, and how to put it right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Whether the problem stops the text compiling.
    pub severity: Severity,
    /// What is wrong, in one line: "use `==` to compare".
    pub message: String,
    /// Where it is.
    pub span: Span,
    /// A word shown under the place itself, such as "did you mean `status`?". It can be empty.
    pub label: String,
    /// Other places that help explain the problem.
    pub related: Vec<Label>,
    /// How to put it right, when the message and the label do not say.
    pub help: Vec<String>,
}

impl Diagnostic {
    /// An error at `span`.
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, span, message.into())
    }

    /// A warning at `span`.
    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, span, message.into())
    }

    fn new(severity: Severity, span: Span, message: String) -> Diagnostic {
        Diagnostic {
            severity,
            message,
            span,
            label: String::new(),
            related: Vec::new(),
            help: Vec::new(),
        }
    }

    /// The diagnostic with a word shown under its place.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = label.into();
        self
    }

    /// The diagnostic with another place it concerns.
    #[must_use]
    pub fn with_related(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.related.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    /// The diagnostic with a line of help on putting it right.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help.push(help.into());
        self
    }

    /// The diagnostic as it is shown to a person: its message, then its lines of the text with the places marked.
    ///
    /// ```text
    /// error: use `==` to compare
    ///   ┌─ open.bcql:2:14
    ///   │
    /// 2 │ where status = "open"
    ///   │              ^ `=` gives a column a value; `==` compares two values
    /// ```
    ///
    /// With `color`, the text holds the terminal codes that colour it.
    pub fn render(&self, file: &SourceFile<'_>, color: bool) -> String {
        let paint = Paint::new(color);
        let accent = match self.severity {
            Severity::Error => paint.red,
            Severity::Warning => paint.yellow,
        };
        let (blue, bold, reset) = (paint.blue, paint.bold, paint.reset);

        let mut labels = vec![(self.span, self.label.as_str(), true)];
        labels.extend(
            self.related
                .iter()
                .map(|label| (label.span, label.message.as_str(), false)),
        );
        let mut lines: Vec<usize> = labels
            .iter()
            .map(|(span, _, _)| file.line_of(span.start as usize))
            .collect();
        lines.sort_unstable();
        lines.dedup();
        let width = (lines.last().copied().unwrap_or(0) + 1).to_string().len();
        let pad = " ".repeat(width + 1);

        let mut out = String::new();
        let (line, column) = file.location(self.span.start);
        out.push_str(&format!(
            "{accent}{bold}{}{reset}{bold}: {}{reset}\n",
            self.severity.name(),
            self.message
        ));
        out.push_str(&format!(
            "{pad}{blue}┌─{reset} {}:{line}:{column}\n",
            file.name
        ));
        out.push_str(&format!("{pad}{blue}│{reset}\n"));

        let mut previous: Option<usize> = None;
        for &index in &lines {
            if previous.is_some_and(|previous| index > previous + 1) {
                out.push_str(&format!("{pad}{blue}·{reset}\n"));
            }
            previous = Some(index);
            let (start, text) = file.line(index);
            let shown = shown(text);
            let number = format!("{:>width$}", index + 1);
            if shown.is_empty() {
                out.push_str(&format!("{blue}{number} │{reset}\n"));
            } else {
                out.push_str(&format!("{blue}{number} │{reset} {shown}\n"));
            }
            for &(span, message, primary) in &labels {
                if file.line_of(span.start as usize) != index {
                    continue;
                }
                let from = (span.start as usize).clamp(start, start + text.len()) - start;
                let to = (span.end as usize).clamp(start + from, start + text.len()) - start;
                let before = width_of(text, 0, from);
                let marks = width_of(text, from, to).max(1);
                let (mark, colour) = if primary { ("^", accent) } else { ("-", blue) };
                let mut row = format!(
                    "{pad}{blue}│{reset} {}{colour}{}",
                    " ".repeat(before),
                    mark.repeat(marks)
                );
                if !message.is_empty() {
                    row.push(' ');
                    row.push_str(message);
                }
                row.push_str(reset);
                row.push('\n');
                out.push_str(&row);
            }
        }

        for help in &self.help {
            for (n, line) in help.lines().enumerate() {
                if n == 0 {
                    out.push_str(&format!("{pad}= {bold}help{reset}: {line}\n"));
                } else if line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&format!("{pad}        {line}\n"));
                }
            }
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity.name(), self.message)
    }
}

/// A text and its name, to show diagnostics in.
#[derive(Debug, Clone)]
pub struct SourceFile<'a> {
    name: &'a str,
    text: &'a str,
    /// Where each line starts.
    line_starts: Vec<usize>,
}

impl<'a> SourceFile<'a> {
    /// A text, with the name diagnostics show for it, such as its file's path.
    pub fn new(name: &'a str, text: &'a str) -> SourceFile<'a> {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        SourceFile {
            name,
            text,
            line_starts,
        }
    }

    /// The line and column, both counted from 1, of a byte of the text. The column counts characters.
    pub fn location(&self, offset: u32) -> (usize, usize) {
        let offset = self.boundary(offset as usize);
        let line = self.line_of(offset);
        let column = self.text[self.line_starts[line]..offset].chars().count() + 1;
        (line + 1, column)
    }

    /// The line, counted from 0, that holds a byte of the text.
    fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Where line `index` starts, and its text without the line break.
    fn line(&self, index: usize) -> (usize, &'a str) {
        let start = self.line_starts[index];
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.text.len(), |next| next - 1);
        let text = &self.text[start..end];
        (start, text.strip_suffix('\r').unwrap_or(text))
    }

    /// The offset, moved back to the start of its character if it falls inside one, and kept within the text.
    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

/// How wide a tab is shown.
const TAB: &str = "    ";

/// A line as it is shown: tabs as spaces, and characters that would change the terminal or hide text replaced.
fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push_str(TAB),
            c if is_unsafe(c) => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

/// How many columns the bytes `from..to` of a line take when shown.
fn width_of(text: &str, from: usize, to: usize) -> usize {
    text.char_indices()
        .filter(|&(at, _)| at >= from && at < to)
        .map(|(_, c)| char_width(c))
        .sum()
}

fn char_width(c: char) -> usize {
    match c {
        '\t' => TAB.len(),
        c if is_unsafe(c) => 1,
        c if is_zero_width(c) => 0,
        c if is_wide(c) => 2,
        _ => 1,
    }
}

/// Control characters, and the characters that change the order text is shown in.
fn is_unsafe(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Accents written over the character before them, and joiners.
fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{200b}'..='\u{200f}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{fe20}'..='\u{fe2f}'
    )
}

/// Characters a terminal shows two columns wide: Chinese, Japanese and Korean writing, and emoji.
fn is_wide(c: char) -> bool {
    matches!(
        c,
        '\u{1100}'..='\u{115f}'
            | '\u{2e80}'..='\u{303e}'
            | '\u{3041}'..='\u{33ff}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{a000}'..='\u{a4cf}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe30}'..='\u{fe4f}'
            | '\u{ff00}'..='\u{ff60}'
            | '\u{ffe0}'..='\u{ffe6}'
            | '\u{1f300}'..='\u{1f64f}'
            | '\u{1f900}'..='\u{1f9ff}'
            | '\u{20000}'..='\u{2fffd}'
            | '\u{30000}'..='\u{3fffd}'
    )
}

/// The terminal codes for colours, or empty ones when colour is off.
struct Paint {
    red: &'static str,
    yellow: &'static str,
    blue: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Paint {
    fn new(color: bool) -> Paint {
        if color {
            Paint {
                red: "\x1b[31m",
                yellow: "\x1b[33m",
                blue: "\x1b[34m",
                bold: "\x1b[1m",
                reset: "\x1b[0m",
            }
        } else {
            Paint {
                red: "",
                yellow: "",
                blue: "",
                bold: "",
                reset: "",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, part: &str) -> Span {
        let start = text.find(part).expect("the part is in the text");
        Span::new(start, start + part.len())
    }

    #[test]
    fn a_problem_is_shown_under_its_line() {
        let text = "from cases\nwhere stauts == \"open\"\n";
        let file = SourceFile::new("open.bcql", text);
        let problem =
            Diagnostic::error(at(text, "stauts"), "there is no column `stauts` in `cases`")
                .with_label("did you mean `status`?");
        assert_eq!(
            problem.render(&file, false),
            "\
error: there is no column `stauts` in `cases`
  ┌─ open.bcql:2:7
  │
2 │ where stauts == \"open\"
  │       ^^^^^^ did you mean `status`?
"
        );
    }

    #[test]
    fn other_places_and_help_are_shown_too() {
        let text = "from cases\nwhere band in [\"a\",\n    \"b\"\n\n\n\n\n\n\nsort risk\n";
        let file = SourceFile::new("q.bcql", text);
        let problem = Diagnostic::error(at(text, "["), "this `[` is never closed")
            .with_label("the list starts here")
            .with_related(at(text, "sort"), "the next step starts here")
            .with_help("close the list with `]`\nafter its last value");
        assert_eq!(
            problem.render(&file, false),
            "\
error: this `[` is never closed
   ┌─ q.bcql:2:15
   │
 2 │ where band in [\"a\",
   │               ^ the list starts here
   ·
10 │ sort risk
   │ ---- the next step starts here
   = help: close the list with `]`
           after its last value
"
        );
    }

    #[test]
    fn a_missing_part_is_marked_where_it_belongs() {
        let text = "from cases\nwhere\n";
        let file = SourceFile::new("q.bcql", text);
        let problem = Diagnostic::error(Span::empty(16), "expected a condition after `where`");
        assert_eq!(
            problem.render(&file, false),
            "\
error: expected a condition after `where`
  ┌─ q.bcql:2:6
  │
2 │ where
  │      ^
"
        );
    }

    #[test]
    fn tabs_wide_characters_and_hidden_ones_keep_the_marks_in_line() {
        let text = "\twhere 名前 == x\u{202e}\n";
        let file = SourceFile::new("q.bcql", text);
        let problem = Diagnostic::warning(at(text, "x"), "a warning");
        let shown = problem.render(&file, false);
        assert!(shown.starts_with("warning: a warning\n"));
        assert!(shown.contains("1 │     where 名前 == x\u{fffd}\n"));
        // A tab shows as 4 columns, and each of the two characters of 名前 as 2.
        let before_x = 4 + "where ".len() + 4 + " == ".len();
        assert!(
            shown.contains(&format!("  │ {}^\n", " ".repeat(before_x))),
            "{shown}"
        );
        assert_eq!(file.location(at(text, "x").start), (1, 14));
    }

    #[test]
    fn windows_line_endings_are_not_shown() {
        let text = "from cases\r\nwhere x = 1\r\n";
        let file = SourceFile::new("q.bcql", text);
        let problem = Diagnostic::error(at(text, "="), "use `==` to compare");
        assert!(
            problem
                .render(&file, false)
                .contains("2 │ where x = 1\n  │         ^\n")
        );
    }

    #[test]
    fn colour_codes_are_added_only_when_asked_for() {
        let text = "from cases\n";
        let file = SourceFile::new("q.bcql", text);
        let problem = Diagnostic::error(at(text, "cases"), "a problem");
        assert!(!problem.render(&file, false).contains('\x1b'));
        assert!(
            problem
                .render(&file, true)
                .starts_with("\x1b[31m\x1b[1merror\x1b[0m\x1b[1m: a problem\x1b[0m\n")
        );
        assert_eq!(problem.to_string(), "error: a problem");
    }
}
