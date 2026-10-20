//! Splits ButtercupQL source text into tokens.
//!
//! Line breaks and indentation are not tokens. Each token records whether it is the first on its line and the column it
//! starts at, and the parser decides what a line break means, since one inside brackets does not end a step.

use std::cmp::Ordering;

use crate::diagnostic::Diagnostic;
use crate::span::Span;

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TokenKind {
    /// A name, such as `cases` or `risk`. The step words, such as `where`, are names too: they start a step only at
    /// the start of a line.
    Name,
    /// A name in backticks, for a name that is not a plain word: `` `order date` ``.
    QuotedName,
    /// A whole number: `20`, `1_000_000`, `0xff`.
    Int,
    /// A number with a fraction or an exponent: `3.5`, `1e6`.
    Float,
    /// A string: `"open"`, `'a'`, `"""text"""`, `r"\d+"`.
    Str,
    /// An f-string: `f"{first} {last}"`.
    FString,
    And,
    As,
    Def,
    Del,
    Else,
    False,
    From,
    If,
    Import,
    In,
    Is,
    None,
    Not,
    Or,
    Pass,
    True,
    /// A Python keyword that ButtercupQL does not use, such as `for`, `lambda` or `return`.
    Reserved,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Dot,
    /// `->`
    Arrow,
    Plus,
    Minus,
    Star,
    Slash,
    /// `//`
    DoubleSlash,
    Percent,
    /// `**`
    DoubleStar,
    /// `=`
    Assign,
    /// `+=`, `-=` and the other operators that change a value where it is.
    AugAssign,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    /// A character with a meaning in other languages that ButtercupQL spells differently: `&`, `|`, `!`, `<>`, `~`.
    Other,
    /// The end of a line outside brackets. The lexer never makes it: the parser sees it in place of a token that
    /// starts a new line.
    Newline,
    /// The end of the text.
    Eof,
}

/// A token: what it is, where it is, and where it sits on its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
    /// The token is the first on its line. Lines joined with `\` count as one.
    pub(crate) line_start: bool,
    /// The column the token starts at.
    pub(crate) col: Column,
}

/// A column on a line, counted two ways so that indentation can be checked as Python checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Column {
    /// With a tab moving to the next multiple of 8.
    wide: u32,
    /// With a tab counting as one character.
    narrow: u32,
}

impl Column {
    fn after(self, c: char) -> Column {
        match c {
            '\t' => Column {
                wide: (self.wide / 8 + 1) * 8,
                narrow: self.narrow + 1,
            },
            _ => Column {
                wide: self.wide + 1,
                narrow: self.narrow + 1,
            },
        }
    }

    /// How this column compares with `other`, or `None` when tabs and spaces are mixed so that the answer depends on
    /// how wide a tab is.
    pub(crate) fn compare(self, other: Column) -> Option<Ordering> {
        let wide = self.wide.cmp(&other.wide);
        (wide == self.narrow.cmp(&other.narrow)).then_some(wide)
    }
}

/// The tokens of a text, and the problems found in reading them.
pub(crate) struct Lexed {
    pub(crate) tokens: Vec<Token>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// Splits `src[start..end]` into tokens, the last of them [`TokenKind::Eof`]. Spans are offsets into the whole of
/// `src`.
pub(crate) fn lex(src: &str, start: usize, end: usize) -> Lexed {
    let mut lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: start,
        end,
        line_start: true,
        cursor: start,
        cursor_col: Column::default(),
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    };
    if src[start..end].starts_with('\u{feff}') {
        lexer.pos += '\u{feff}'.len_utf8();
        lexer.cursor = lexer.pos;
    }
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        diagnostics: lexer.diagnostics,
    }
}

/// Reports the characters that change the order text is shown in. They can make a query read differently from what
/// it does, so they are refused everywhere, even in strings and comments.
pub(crate) fn hidden_characters(src: &str) -> Vec<Diagnostic> {
    src.char_indices()
        .filter(|&(_, c)| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|(at, c)| {
            Diagnostic::error(
                Span::new(at, at + c.len_utf8()),
                format!(
                    "this text holds a hidden character, U+{:04X}, that changes the order text is shown in",
                    c as u32
                ),
            )
            .with_help("remove it: it can make a query read differently from what it does")
        })
        .collect()
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    end: usize,
    /// No token has been read yet on the current line.
    line_start: bool,
    /// The last place a column was worked out, and that column, so that each column is worked out from the last.
    cursor: usize,
    cursor_col: Column,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while let Some(start) = self.skip_space() {
            if let Some(kind) = self.scan(start) {
                self.push(kind, start);
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::empty(self.end),
            line_start: true,
            col: Column::default(),
        });
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let col = self.column_at(start);
        let line_start = std::mem::replace(&mut self.line_start, false);
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
            line_start,
            col,
        });
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    /// The byte at `at`, or 0 past the end.
    fn byte(&self, at: usize) -> u8 {
        if at < self.end { self.bytes[at] } else { 0 }
    }

    /// The column of byte `at`, worked out from the last column found.
    fn column_at(&mut self, at: usize) -> Column {
        let (begin, mut col) = match self.src[self.cursor..at].rfind('\n') {
            Some(newline) => (self.cursor + newline + 1, Column::default()),
            None => (self.cursor, self.cursor_col),
        };
        for c in self.src[begin..at].chars() {
            col = col.after(c);
        }
        self.cursor = at;
        self.cursor_col = col;
        col
    }

    /// Skips spaces, comments and line breaks, and returns where the next token starts.
    fn skip_space(&mut self) -> Option<usize> {
        while self.pos < self.end {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\x0c' | b'\r' => self.pos += 1,
                b'\n' => {
                    self.pos += 1;
                    self.line_start = true;
                }
                b'#' => {
                    while self.pos < self.end && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'\\' => {
                    // A backslash at the end of a line joins the next line to it.
                    let mut next = self.pos + 1;
                    if self.byte(next) == b'\r' {
                        next += 1;
                    }
                    if self.byte(next) == b'\n' {
                        self.pos = next + 1;
                    } else if next >= self.end {
                        self.error(
                            Span::new(self.pos, self.pos + 1),
                            "a `\\` at the end of the text has no line to join",
                        );
                        self.pos = self.end;
                    } else {
                        return Some(self.pos);
                    }
                }
                _ => return Some(self.pos),
            }
        }
        None
    }

    /// Reads the token that starts at `start`, or reports a character that starts none and returns `None`.
    fn scan(&mut self, start: usize) -> Option<TokenKind> {
        use TokenKind as T;
        let b = self.bytes[start];
        let next = self.byte(start + 1);
        let after = self.byte(start + 2);
        let (kind, len) = match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => return Some(self.word(start)),
            b'0'..=b'9' => return Some(self.number(start)),
            b'.' if next.is_ascii_digit() => return Some(self.number(start)),
            b'"' | b'\'' => return Some(self.string(start, start)),
            b'`' => return Some(self.quoted_name(start)),
            b'(' => (T::LParen, 1),
            b')' => (T::RParen, 1),
            b'[' => (T::LBracket, 1),
            b']' => (T::RBracket, 1),
            b'{' => (T::LBrace, 1),
            b'}' => (T::RBrace, 1),
            b',' => (T::Comma, 1),
            b';' => (T::Semicolon, 1),
            b'.' => (T::Dot, 1),
            b':' if next == b'=' => (T::Other, 2),
            b':' => (T::Colon, 1),
            b'-' if next == b'>' => (T::Arrow, 2),
            b'+' | b'-' | b'%' if next == b'=' => (T::AugAssign, 2),
            b'+' => (T::Plus, 1),
            b'-' => (T::Minus, 1),
            b'%' => (T::Percent, 1),
            b'*' | b'/' if next == b && after == b'=' => (T::AugAssign, 3),
            b'*' | b'/' if next == b'=' => (T::AugAssign, 2),
            b'*' if next == b'*' => (T::DoubleStar, 2),
            b'*' => (T::Star, 1),
            b'/' if next == b'/' => (T::DoubleSlash, 2),
            b'/' => (T::Slash, 1),
            b'=' if next == b'=' => (T::EqEq, 2),
            b'=' => (T::Assign, 1),
            b'!' if next == b'=' => (T::NotEq, 2),
            b'<' if next == b'=' => (T::LtEq, 2),
            b'<' if next == b'>' || next == b'<' => (T::Other, 2),
            b'<' => (T::Lt, 1),
            b'>' if next == b'=' => (T::GtEq, 2),
            b'>' if next == b'>' => (T::Other, 2),
            b'>' => (T::Gt, 1),
            b'&' | b'|' if next == b => (T::Other, 2),
            b'!' | b'&' | b'|' | b'~' | b'^' | b'@' | b'$' | b'?' => (T::Other, 1),
            b'\\' => {
                self.pos = start + 1;
                self.error(
                    Span::new(start, start + 1),
                    "a `\\` joins two lines only as the last character of a line",
                );
                return None;
            }
            _ => {
                let c = self.src[start..].chars().next().unwrap_or('\u{fffd}');
                if c.is_alphabetic() {
                    return Some(self.word(start));
                }
                self.pos = start + c.len_utf8();
                self.error(
                    Span::new(start, self.pos),
                    format!("unexpected character {}", describe_char(c)),
                );
                return None;
            }
        };
        self.pos = start + len;
        Some(kind)
    }

    /// Reads a name or a keyword, or a string with a prefix such as `r` or `f`.
    fn word(&mut self, start: usize) -> TokenKind {
        let mut pos = start;
        while pos < self.end {
            let b = self.bytes[pos];
            if b.is_ascii_alphanumeric() || b == b'_' {
                pos += 1;
                continue;
            }
            if b < 0x80 {
                break;
            }
            let c = self.src[pos..].chars().next().unwrap_or(' ');
            if !(c.is_alphanumeric() || is_joining_mark(c)) {
                break;
            }
            pos += c.len_utf8();
        }
        self.pos = pos;
        let text = &self.src[start..pos];
        if matches!(self.byte(pos), b'"' | b'\'')
            && text.len() <= 2
            && text.bytes().all(|b| b"rRuUfFbBtT".contains(&b))
        {
            return self.string(start, pos);
        }
        keyword(text).unwrap_or(TokenKind::Name)
    }

    /// Reads a number: `20`, `1_000_000`, `3.5`, `.5`, `1e6`, `0xff`, `0o17`, `0b101`.
    fn number(&mut self, start: usize) -> TokenKind {
        let radix = match (self.bytes[start], self.byte(start + 1) | 0x20) {
            (b'0', b'x') => 16,
            (b'0', b'o') => 8,
            (b'0', b'b') => 2,
            _ => 10,
        };
        if radix != 10 {
            let mut pos = start + 2;
            while pos < self.end
                && (self.bytes[pos].is_ascii_alphanumeric() || self.bytes[pos] == b'_')
            {
                pos += 1;
            }
            self.pos = pos;
            let digits = &self.src[start + 2..pos];
            let valid = !digits.is_empty()
                && digits.chars().all(|c| c == '_' || c.is_digit(radix))
                && underscores_between_digits(digits, true);
            if !valid {
                let text = &self.src[start..pos];
                self.error(
                    Span::new(start, pos),
                    format!("`{text}` is not a valid number"),
                );
            }
            return TokenKind::Int;
        }

        let mut kind = TokenKind::Int;
        let mut pos = self.digits(start);
        if self.byte(pos) == b'.' {
            kind = TokenKind::Float;
            pos = self.digits(pos + 1);
        }
        if self.byte(pos) | 0x20 == b'e' {
            let mut exponent = pos + 1;
            if matches!(self.byte(exponent), b'+' | b'-') {
                exponent += 1;
            }
            if self.byte(exponent).is_ascii_digit() {
                kind = TokenKind::Float;
                pos = self.digits(exponent);
            }
        }
        if self.byte(pos) | 0x20 == b'j' {
            self.pos = pos + 1;
            self.error(
                Span::new(start, self.pos),
                "complex numbers are not supported",
            );
            return kind;
        }
        let mut trailing = false;
        while pos < self.end && (self.bytes[pos].is_ascii_alphanumeric() || self.bytes[pos] == b'_')
        {
            pos += 1;
            trailing = true;
        }
        self.pos = pos;
        let span = Span::new(start, pos);
        let text = &self.src[start..pos];
        if trailing {
            self.error(span, format!("`{text}` is not a valid number"));
        } else if !underscores_between_digits(text, false) {
            self.diagnostics.push(
                Diagnostic::error(span, format!("`{text}` is not a valid number"))
                    .with_help("an `_` in a number goes between two digits, as in `1_000_000`"),
            );
        } else if kind == TokenKind::Int
            && text.starts_with('0')
            && text.bytes().any(|b| matches!(b, b'1'..=b'9'))
        {
            self.diagnostics.push(
                Diagnostic::error(span, "a whole number can't start with `0`")
                    .with_help("write an octal number with `0o` first, as in `0o17`"),
            );
        }
        kind
    }

    /// The end of the digits and underscores from `pos`.
    fn digits(&self, mut pos: usize) -> usize {
        while self.byte(pos).is_ascii_digit() || self.byte(pos) == b'_' {
            pos += 1;
        }
        pos
    }

    /// Reads a string or an f-string whose prefix starts at `start` and whose quote is at `quote_at`.
    fn string(&mut self, start: usize, quote_at: usize) -> TokenKind {
        let prefix = self.src[start..quote_at].to_ascii_lowercase();
        let quote = self.bytes[quote_at];
        let triple = self.byte(quote_at + 1) == quote && self.byte(quote_at + 2) == quote;
        let body = quote_at + if triple { 3 } else { 1 };
        let formatted = prefix.contains('f');
        let (end, closed) = if formatted {
            let scan = scan_fstring(self.src, body, self.end, quote, triple);
            for (span, message) in scan.problems {
                self.error(span, message);
            }
            (scan.end, scan.closed)
        } else {
            scan_plain(self.bytes, body, self.end, quote, triple)
        };
        self.pos = end;
        if !closed {
            let quotes = &self.src[quote_at..body];
            let message = if triple {
                "this string is never closed"
            } else {
                "this string is not closed on its line"
            };
            self.diagnostics.push(
                Diagnostic::error(Span::new(start, body), message)
                    .with_label("the string starts here")
                    .with_help(format!("end it with {quotes}")),
            );
        }
        match prefix.as_str() {
            "" | "r" | "u" | "f" | "rf" | "fr" => {}
            "b" | "br" | "rb" => self.error(
                Span::new(start, quote_at),
                "bytes values are not supported: remove the `b`",
            ),
            _ => {
                let written = &self.src[start..quote_at];
                self.diagnostics.push(
                    Diagnostic::error(
                        Span::new(start, quote_at),
                        format!("`{written}` is not a string prefix"),
                    )
                    .with_help("use `r` for a raw string, or `f` for an f-string"),
                );
            }
        }
        if formatted {
            TokenKind::FString
        } else {
            TokenKind::Str
        }
    }

    /// Reads a name in backticks: `` `order date` ``. Two backticks stand for one inside it.
    fn quoted_name(&mut self, start: usize) -> TokenKind {
        let mut pos = start + 1;
        loop {
            if pos >= self.end || self.bytes[pos] == b'\n' {
                self.diagnostics.push(
                    Diagnostic::error(
                        Span::new(start, start + 1),
                        "this name in backticks is never closed",
                    )
                    .with_help("end the name with a backtick, as in `` `order date` ``"),
                );
                break;
            }
            if self.bytes[pos] == b'`' {
                if self.byte(pos + 1) == b'`' && pos > start + 1 {
                    pos += 2;
                    continue;
                }
                pos += 1;
                if pos == start + 2 {
                    self.error(Span::new(start, pos), "a name in backticks can't be empty");
                }
                break;
            }
            pos += 1;
        }
        self.pos = pos;
        TokenKind::QuotedName
    }
}

/// Python's keywords, and what ButtercupQL makes of each.
fn keyword(text: &str) -> Option<TokenKind> {
    use TokenKind as T;
    Some(match text {
        "and" => T::And,
        "as" => T::As,
        "def" => T::Def,
        "del" => T::Del,
        "else" => T::Else,
        "False" => T::False,
        "from" => T::From,
        "if" => T::If,
        "import" => T::Import,
        "in" => T::In,
        "is" => T::Is,
        "None" => T::None,
        "not" => T::Not,
        "or" => T::Or,
        "pass" => T::Pass,
        "True" => T::True,
        "assert" | "async" | "await" | "break" | "class" | "continue" | "elif" | "except"
        | "finally" | "for" | "global" | "lambda" | "nonlocal" | "raise" | "return" | "try"
        | "while" | "with" | "yield" => T::Reserved,
        _ => return Option::None,
    })
}

/// The marks that can follow a letter inside a name: accents written as their own character, and joiners.
fn is_joining_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036f}' | '\u{200c}' | '\u{200d}')
}

/// Whether every `_` in a number sits between two digits. A number with a base, such as `0x_ff`, may also start its
/// digits with one.
fn underscores_between_digits(text: &str, after_base: bool) -> bool {
    let bytes = text.as_bytes();
    let digit = |at: usize| {
        bytes.get(at).is_some_and(|b| {
            if after_base {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        })
    };
    bytes.iter().enumerate().all(|(at, &b)| {
        let after_digit = (at > 0 && digit(at - 1)) || (after_base && at == 0);
        b != b'_' || (after_digit && digit(at + 1))
    })
}

/// A character, written so that it can be seen in a message.
fn describe_char(c: char) -> String {
    if c.is_control() || c.is_whitespace() {
        format!("U+{:04X}", c as u32)
    } else {
        format!("`{c}` (U+{:04X})", c as u32)
    }
}

/// Finds the end of a string's body, from just after its opening quotes. Returns where the string ends, after its
/// closing quotes, and whether it is closed; an unclosed string ends at the end of its line, or of the text.
pub(crate) fn scan_plain(
    bytes: &[u8],
    mut pos: usize,
    end: usize,
    quote: u8,
    triple: bool,
) -> (usize, bool) {
    while pos < end {
        let b = bytes[pos];
        if b == b'\\' {
            // An escaped character never ends the string, nor does a line break after a backslash.
            pos += if bytes.get(pos + 1) == Some(&b'\r') && bytes.get(pos + 2) == Some(&b'\n') {
                3
            } else {
                2
            };
        } else if b == quote {
            if !triple {
                return (pos + 1, true);
            }
            if pos + 2 < end && bytes[pos + 1] == quote && bytes[pos + 2] == quote {
                return (pos + 3, true);
            }
            pos += 1;
        } else if b == b'\n' && !triple {
            return (pos, false);
        } else {
            pos += 1;
        }
    }
    (end, false)
}

/// A part of an f-string's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Text, as written: its escapes and doubled braces are still to be read.
    Text(Span),
    /// A `{field}`: the expression, and anything after it such as `!r`, `:.2f` or `=`.
    Field { expr: Span, extra: Option<Span> },
}

/// What reading an f-string found.
pub(crate) struct FScan {
    /// The text and the fields of the body, in order.
    pub(crate) pieces: Vec<Piece>,
    /// Where the string ends, after the closing quotes.
    pub(crate) end: usize,
    pub(crate) closed: bool,
    pub(crate) problems: Vec<(Span, String)>,
}

/// Reads an f-string's body, from just after its opening quotes, into its text and its fields.
pub(crate) fn scan_fstring(src: &str, body: usize, end: usize, quote: u8, triple: bool) -> FScan {
    let bytes = src.as_bytes();
    let mut scan = FScan {
        pieces: Vec::new(),
        end,
        closed: false,
        problems: Vec::new(),
    };
    let mut pos = body;
    let mut text = body;
    while pos < end {
        let b = bytes[pos];
        let next = if pos + 1 < end { bytes[pos + 1] } else { 0 };
        if b == b'\\' {
            pos += if next == b'\r' && bytes.get(pos + 2) == Some(&b'\n') {
                3
            } else {
                2
            };
        } else if b == quote
            && (!triple || (pos + 2 < end && next == quote && bytes[pos + 2] == quote))
        {
            push_text(&mut scan.pieces, text, pos);
            scan.end = pos + if triple { 3 } else { 1 };
            scan.closed = true;
            return scan;
        } else if b == b'\n' && !triple {
            break;
        } else if (b == b'{' || b == b'}') && next == b {
            pos += 2;
        } else if b == b'}' {
            scan.problems.push((
                Span::new(pos, pos + 1),
                "a single `}` in an f-string has no `{`: write `}}` for a brace".to_string(),
            ));
            pos += 1;
        } else if b == b'{' {
            push_text(&mut scan.pieces, text, pos);
            match scan_field(bytes, pos + 1, end, triple) {
                Ok((expr, extra, close)) => {
                    scan.pieces.push(Piece::Field { expr, extra });
                    pos = close + 1;
                    text = pos;
                }
                Err(stop) => {
                    scan.problems.push((
                        Span::new(pos, pos + 1),
                        "this `{` is never closed: a field in an f-string ends with `}`"
                            .to_string(),
                    ));
                    // The rest of the string can't be read; it ends where the field gave up.
                    scan.end = stop;
                    scan.closed = true;
                    return scan;
                }
            }
        } else {
            pos += 1;
        }
    }
    let pos = pos.min(end);
    push_text(&mut scan.pieces, text, pos);
    scan.end = pos;
    scan
}

fn push_text(pieces: &mut Vec<Piece>, from: usize, to: usize) {
    if from < to {
        pieces.push(Piece::Text(Span::new(from, to)));
    }
}

/// Reads a field of an f-string, from just after its `{`. Returns the expression's span, the span of anything after
/// it, and where the closing `}` is; or, when the field is not closed, where reading it stopped.
fn scan_field(
    bytes: &[u8],
    start: usize,
    end: usize,
    triple: bool,
) -> Result<(Span, Option<Span>, usize), usize> {
    let mut depth = 0u32;
    let mut extra: Option<usize> = None;
    let mut pos = start;
    while pos < end {
        let b = bytes[pos];
        match b {
            b'}' if depth == 0 => {
                let expr = Span::new(start, extra.unwrap_or(pos));
                return Ok((expr, extra.map(|from| Span::new(from, pos)), pos));
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'\'' | b'"' if extra.is_none() => {
                // A string inside the field.
                let inner_triple = pos + 2 < end && bytes[pos + 1] == b && bytes[pos + 2] == b;
                let body = pos + if inner_triple { 3 } else { 1 };
                let (after, closed) = scan_plain(bytes, body, end, b, inner_triple);
                if !closed {
                    return Err(after);
                }
                pos = after;
                continue;
            }
            b'\n' if !triple => return Err(pos),
            b'!' if depth == 0 && extra.is_none() && bytes.get(pos + 1) != Some(&b'=') => {
                extra = Some(pos)
            }
            b':' if depth == 0 && extra.is_none() => extra = Some(pos),
            b'=' if depth == 0 && extra.is_none() && is_debug_equals(bytes, start, pos, end) => {
                extra = Some(pos)
            }
            _ => {}
        }
        pos += 1;
    }
    Err(end)
}

/// Whether the `=` at `at` is the `=` of `f"{x=}"`, which shows an expression with its value, rather than part of
/// `==`, `!=`, `<=` or `>=`.
fn is_debug_equals(bytes: &[u8], start: usize, at: usize, end: usize) -> bool {
    if at > start && matches!(bytes[at - 1], b'=' | b'!' | b'<' | b'>') {
        return false;
    }
    let mut next = at + 1;
    if next < end && bytes[next] == b'=' {
        return false;
    }
    while next < end && bytes[next] == b' ' {
        next += 1;
    }
    next < end && matches!(bytes[next], b'}' | b'!' | b':')
}

/// Reads the escapes in a string's text, as Python does. In an f-string's text, `{{` and `}}` stand for one brace.
/// Returns the text, and the problems found, each with its place.
pub(crate) fn unescape(
    src: &str,
    span: Span,
    raw: bool,
    fstring: bool,
) -> (String, Vec<(Span, String)>) {
    let text = &src[span.range()];
    let base = span.start as usize;
    let mut out = String::with_capacity(text.len());
    let mut problems = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\r' if chars.peek().is_some_and(|&(_, next)| next == '\n') => {}
            '{' | '}' if fstring => {
                out.push(c);
                chars.next();
            }
            '\\' if !raw => {
                let Some((_, escape)) = chars.next() else {
                    break;
                };
                let mut end = at + 1 + escape.len_utf8();
                match escape {
                    '\n' => {}
                    '\r' => {
                        if chars.peek().is_some_and(|&(_, next)| next == '\n') {
                            chars.next();
                        }
                    }
                    '\\' | '\'' | '"' => out.push(escape),
                    'a' => out.push('\u{7}'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'v' => out.push('\u{b}'),
                    '0'..='7' => {
                        let mut value = escape.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match chars.peek() {
                                Some(&(_, digit @ '0'..='7')) => {
                                    value = value * 8 + digit.to_digit(8).unwrap_or(0);
                                    chars.next();
                                }
                                _ => break,
                            }
                        }
                        out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                    }
                    'x' | 'u' | 'U' => {
                        let wanted = match escape {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut digits = String::new();
                        while digits.len() < wanted {
                            match chars.peek() {
                                Some(&(_, digit)) if digit.is_ascii_hexdigit() => {
                                    digits.push(digit);
                                    chars.next();
                                }
                                _ => break,
                            }
                        }
                        end += digits.len();
                        let value = u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32);
                        match value {
                            Some(c) if digits.len() == wanted => out.push(c),
                            _ if digits.len() < wanted => problems.push((
                                Span::new(base + at, base + end),
                                format!("`\\{escape}` takes {wanted} hexadecimal digits"),
                            )),
                            _ => problems.push((
                                Span::new(base + at, base + end),
                                format!("`\\{escape}{digits}` is not a Unicode character"),
                            )),
                        }
                    }
                    'N' => problems.push((
                        Span::new(base + at, base + end),
                        "`\\N{...}` is not supported: write the character itself, or `\\u` and its code".to_string(),
                    )),
                    other => problems.push((
                        Span::new(base + at, base + end),
                        format!(
                            "`\\{other}` is not an escape: write `\\\\` for a backslash, or put `r` before the string to keep backslashes as they are"
                        ),
                    )),
                }
            }
            _ => out.push(c),
        }
    }
    (out, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind as T;

    fn tokens(src: &str) -> Vec<(TokenKind, &str)> {
        let lexed = lex(src, 0, src.len());
        assert!(lexed.diagnostics.is_empty(), "{:#?}", lexed.diagnostics);
        lexed
            .tokens
            .iter()
            .filter(|t| t.kind != T::Eof)
            .map(|t| (t.kind, &src[t.span.range()]))
            .collect()
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokens(src).into_iter().map(|(kind, _)| kind).collect()
    }

    fn problems(src: &str) -> Vec<String> {
        lex(src, 0, src.len())
            .diagnostics
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn names_keywords_and_step_words() {
        assert_eq!(
            tokens("from cases where risk is not None"),
            vec![
                (T::From, "from"),
                (T::Name, "cases"),
                (T::Name, "where"),
                (T::Name, "risk"),
                (T::Is, "is"),
                (T::Not, "not"),
                (T::None, "None"),
            ]
        );
        assert_eq!(kinds("lambda for return"), vec![T::Reserved; 3]);
        assert_eq!(
            tokens("café naïve"),
            vec![(T::Name, "café"), (T::Name, "naïve")]
        );
        assert_eq!(
            tokens("`order date`"),
            vec![(T::QuotedName, "`order date`")]
        );
    }

    #[test]
    fn numbers_as_python_writes_them() {
        assert_eq!(
            kinds("20 1_000_000 0xff 0o17 0b101 0x_ff 3.5 .5 5. 1e6 1.5e-3 0 00"),
            vec![
                T::Int,
                T::Int,
                T::Int,
                T::Int,
                T::Int,
                T::Int,
                T::Float,
                T::Float,
                T::Float,
                T::Float,
                T::Float,
                T::Int,
                T::Int,
            ]
        );
        assert_eq!(problems("1__0"), vec!["`1__0` is not a valid number"]);
        assert_eq!(problems("1_"), vec!["`1_` is not a valid number"]);
        assert_eq!(problems("007"), vec!["a whole number can't start with `0`"]);
        assert_eq!(problems("12abc"), vec!["`12abc` is not a valid number"]);
        assert_eq!(problems("0x"), vec!["`0x` is not a valid number"]);
        assert_eq!(problems("0b102"), vec!["`0b102` is not a valid number"]);
        assert_eq!(problems("3j"), vec!["complex numbers are not supported"]);
    }

    #[test]
    fn operators_take_the_longest_match() {
        assert_eq!(
            kinds("== != <= >= < > = += -= *= /= //= %= **= ** // -> + - * / %"),
            vec![
                T::EqEq,
                T::NotEq,
                T::LtEq,
                T::GtEq,
                T::Lt,
                T::Gt,
                T::Assign,
                T::AugAssign,
                T::AugAssign,
                T::AugAssign,
                T::AugAssign,
                T::AugAssign,
                T::AugAssign,
                T::AugAssign,
                T::DoubleStar,
                T::DoubleSlash,
                T::Arrow,
                T::Plus,
                T::Minus,
                T::Star,
                T::Slash,
                T::Percent,
            ]
        );
        assert_eq!(
            tokens("& && | || ! ~ <> :="),
            vec![
                (T::Other, "&"),
                (T::Other, "&&"),
                (T::Other, "|"),
                (T::Other, "||"),
                (T::Other, "!"),
                (T::Other, "~"),
                (T::Other, "<>"),
                (T::Other, ":="),
            ]
        );
    }

    #[test]
    fn strings_in_every_python_form() {
        assert_eq!(
            tokens(
                r#""open" 'a' r"\d+" """two
lines""" f"{x}" rf'{y}' u"z""#
            ),
            vec![
                (T::Str, "\"open\""),
                (T::Str, "'a'"),
                (T::Str, "r\"\\d+\""),
                (T::Str, "\"\"\"two\nlines\"\"\""),
                (T::FString, "f\"{x}\""),
                (T::FString, "rf'{y}'"),
                (T::Str, "u\"z\""),
            ]
        );
        assert_eq!(tokens(r#""say \"hi\"""#), vec![(T::Str, r#""say \"hi\"""#)]);
        assert_eq!(
            problems("\"open\nwhere"),
            vec!["this string is not closed on its line"]
        );
        assert_eq!(problems("'''open"), vec!["this string is never closed"]);
        assert_eq!(
            problems("b\"x\""),
            vec!["bytes values are not supported: remove the `b`"]
        );
        assert_eq!(problems("ub\"x\""), vec!["`ub` is not a string prefix"]);
    }

    #[test]
    fn f_strings_split_into_text_and_fields() {
        let src = r#"f"Hi {first} {{x}} {name.upper()!r} {"a" + b} {n:>3}""#;
        let scan = scan_fstring(src, 2, src.len(), b'"', false);
        assert!(scan.closed && scan.problems.is_empty());
        let pieces: Vec<String> = scan
            .pieces
            .iter()
            .map(|piece| match *piece {
                Piece::Text(span) => format!("text {:?}", &src[span.range()]),
                Piece::Field { expr, extra } => {
                    let extra = extra.map_or(String::new(), |span| {
                        format!(" extra {:?}", &src[span.range()])
                    });
                    format!("field {:?}{extra}", &src[expr.range()])
                }
            })
            .collect();
        assert_eq!(
            pieces,
            vec![
                r#"text "Hi ""#,
                r#"field "first""#,
                r#"text " {{x}} ""#,
                r#"field "name.upper()" extra "!r""#,
                r#"text " ""#,
                r#"field "\"a\" + b""#,
                r#"text " ""#,
                r#"field "n" extra ":>3""#,
            ]
        );
        assert_eq!(
            problems("f\"{x\""),
            vec!["this `{` is never closed: a field in an f-string ends with `}`"]
        );
        assert_eq!(
            problems("f\"a}\""),
            vec!["a single `}` in an f-string has no `{`: write `}}` for a brace"]
        );
        let debug = "f\"{x=} {a == b}\"";
        let scan = scan_fstring(debug, 2, debug.len(), b'"', false);
        assert!(matches!(
            scan.pieces[0],
            Piece::Field { extra: Some(_), .. }
        ));
        assert!(matches!(scan.pieces[2], Piece::Field { extra: None, .. }));
    }

    #[test]
    fn escapes_are_read_as_python_reads_them() {
        let read = |text: &str, raw: bool| {
            let (value, problems) = unescape(text, Span::new(0, text.len()), raw, false);
            assert!(problems.is_empty(), "{problems:?}");
            value
        };
        assert_eq!(
            read(r#"a\tb\n\\\'\"\x41\u00e9\U0001F33C\101\0"#, false),
            "a\tb\n\\'\"Aé🌼A\0"
        );
        assert_eq!(read("line \\\nend", false), "line end");
        assert_eq!(read("two\r\nlines", false), "two\nlines");
        assert_eq!(read(r"\d+\.", true), r"\d+\.");
        let bad = r"\q \x4 \N{DASH}";
        let (_, problems) = unescape(bad, Span::new(0, bad.len()), false, false);
        let messages: Vec<&str> = problems.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(messages.len(), 3);
        assert!(messages[0].starts_with("`\\q` is not an escape"));
        assert_eq!(messages[1], "`\\x` takes 2 hexadecimal digits");
        assert!(messages[2].starts_with("`\\N{...}` is not supported"));
        let (braces, _) = unescape("{{x}}", Span::new(0, 5), false, true);
        assert_eq!(braces, "{x}");
    }

    #[test]
    fn each_token_knows_its_line_and_column() {
        let src = "from cases\n\n# a comment\n    where a \\\n  and b\n\tc (d,\n e)";
        let lexed = lex(src, 0, src.len());
        let starts: Vec<(&str, bool, u32)> = lexed
            .tokens
            .iter()
            .filter(|t| t.kind != T::Eof)
            .map(|t| (&src[t.span.range()], t.line_start, t.col.wide))
            .collect();
        assert_eq!(
            starts,
            vec![
                ("from", true, 0),
                ("cases", false, 5),
                ("where", true, 4),
                ("a", false, 10),
                ("and", false, 2),
                ("b", false, 6),
                ("c", true, 8),
                ("(", false, 10),
                ("d", false, 11),
                (",", false, 12),
                ("e", true, 1),
                (")", false, 2),
            ]
        );
    }

    #[test]
    fn indentation_compares_as_python_compares_it() {
        let col = |text: &str| text.chars().fold(Column::default(), Column::after);
        assert_eq!(col("    ").compare(col("  ")), Some(Ordering::Greater));
        assert_eq!(col("\t").compare(col("\t")), Some(Ordering::Equal));
        assert_eq!(col("\t    ").compare(col("\t")), Some(Ordering::Greater));
        // A tab and eight spaces are the same width only if a tab is 8 wide, so Python refuses to compare them.
        assert_eq!(col("\t").compare(col("        ")), None);
    }

    #[test]
    fn windows_line_endings_and_a_byte_order_mark_change_nothing() {
        let unix = "from cases\nwhere a == \"x\"\n";
        let windows = format!("\u{feff}{}", unix.replace('\n', "\r\n"));
        let kinds_of = |src: &str| {
            lex(src, 0, src.len())
                .tokens
                .iter()
                .map(|t| (t.kind, t.line_start, t.col))
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds_of(unix), kinds_of(&windows));
    }

    #[test]
    fn stray_characters_are_reported_and_skipped() {
        let src = "where a € b \\ c";
        let lexed = lex(src, 0, src.len());
        let messages: Vec<&str> = lexed
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            messages,
            vec![
                "unexpected character `€` (U+20AC)",
                "a `\\` joins two lines only as the last character of a line",
            ]
        );
        assert_eq!(lexed.tokens.len(), 5);
    }

    #[test]
    fn hidden_characters_are_found_anywhere() {
        let src = "# note \u{202e}\nwhere a == \"\u{2066}\"";
        let found = hidden_characters(src);
        assert_eq!(found.len(), 2);
        assert!(found[0].message.contains("U+202E"));
    }

    #[test]
    fn names_in_backticks() {
        assert_eq!(tokens("`a``b`"), vec![(T::QuotedName, "`a``b`")]);
        assert_eq!(
            problems("`open"),
            vec!["this name in backticks is never closed"]
        );
        assert_eq!(problems("``"), vec!["a name in backticks can't be empty"]);
    }
}
