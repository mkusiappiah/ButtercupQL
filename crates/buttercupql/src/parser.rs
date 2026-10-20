//! Reads ButtercupQL source text into a syntax tree, reporting every problem it finds.
//!
//! The parser is written by hand. A step is one line, so after a mistake it starts again at the next step and goes on
//! to find the problems after it, and each message can say what the writer most likely meant, including when they
//! wrote SQL or another language's habit.

use std::cmp::Ordering;

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::lexer::{self, Column, Piece, Token, TokenKind as T};
use crate::span::Span;
use crate::suggest;

#[cfg(test)]
mod tests;

/// The words that start a step.
pub(crate) const STEP_WORDS: [&str; 12] = [
    "from",
    "where",
    "select",
    "del",
    "sort",
    "take",
    "skip",
    "distinct",
    "group",
    "summarize",
    "join",
    "left",
];

/// Words from SQL and from data-frame libraries, the step that does their work in ButtercupQL, and an example.
const OTHER_STEPS: [(&str, &str, &str); 18] = [
    ("order", "sort", "sort -risk, raised"),
    ("orderby", "sort", "sort -risk, raised"),
    ("sort_values", "sort", "sort -risk, raised"),
    ("limit", "take", "take 20"),
    ("top", "take", "take 20"),
    ("head", "take", "take 20"),
    ("offset", "skip", "skip 40"),
    ("having", "where", "where n > 100"),
    ("filter", "where", "where status == \"open\""),
    ("drop", "del", "del notes"),
    ("unique", "distinct", "distinct"),
    ("groupby", "group by", "group by operator:"),
    ("aggregate", "summarize", "summarize:"),
    ("agg", "summarize", "summarize:"),
    (
        "inner",
        "join",
        "join accounts on account_id == accounts.id",
    ),
    ("mutate", "a column step", "total = price * quantity"),
    ("assign", "a column step", "total = price * quantity"),
    ("rename", "select", "select account = account_id"),
];

/// A problem was found and reported. The parser starts again at the next step.
struct Fail;

type Parse<X> = Result<X, Fail>;

/// How a list of steps ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// At the end of the file. A line indented less than the first is reported, and read as a step.
    File,
    /// At a line indented less than the steps: the end of a `def`'s body.
    Block,
    /// At the `)` that closes a query in brackets, or at a line indented no more than the line the `(` is on,
    /// whose column this holds.
    Bracket(Column),
}

/// Reads a ButtercupQL text into its syntax tree. The problems found are returned in the order of the text.
pub(crate) fn parse(src: &str) -> (File, Vec<Diagnostic>) {
    let lexed = lexer::lex(src, 0, src.len());
    let mut parser = Parser::new(src, lexed.tokens);
    parser.diagnostics = lexed.diagnostics;
    parser.diagnostics.extend(lexer::hidden_characters(src));
    let file = parser.file();
    let mut diagnostics = parser.diagnostics;
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    (file, diagnostics)
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    /// How many brackets are open in the step being read. Inside brackets, a line break does not end the step.
    brackets: u32,
    /// How many brackets were open when the last problem was found, so that starting again skips their contents.
    failed_in: u32,
    /// The column of the lines being read.
    block: Column,
    /// How many queries in brackets are being read.
    queries: u32,
    /// The token that starts the line being read. It starts a line, but is not the end of one.
    line_head: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        Parser {
            src,
            tokens,
            pos: 0,
            brackets: 0,
            failed_in: 0,
            block: Column::default(),
            queries: 0,
            line_head: usize::MAX,
            diagnostics: Vec::new(),
        }
    }

    // ---- Tokens

    fn tok(&self) -> Token {
        self.tokens[self.pos]
    }

    fn nth(&self, n: usize) -> Token {
        self.tokens[(self.pos + n).min(self.tokens.len() - 1)]
    }

    /// What the token at `index` is, as a step sees it: a token that starts a new line, outside brackets, is the
    /// end of the line.
    fn kind_at(&self, index: usize) -> T {
        let index = index.min(self.tokens.len() - 1);
        let token = self.tokens[index];
        if token.kind != T::Eof && token.line_start && self.brackets == 0 && index != self.line_head
        {
            T::Newline
        } else {
            token.kind
        }
    }

    fn peek(&self) -> T {
        self.kind_at(self.pos)
    }

    fn peek_nth(&self, n: usize) -> T {
        self.kind_at(self.pos + n)
    }

    fn at(&self, kind: T) -> bool {
        self.peek() == kind
    }

    /// Whether the next token is the name `word`.
    fn at_word(&self, word: &str) -> bool {
        self.peek() == T::Name && self.text(self.tok()) == word
    }

    fn at_other(&self, symbol: &str) -> bool {
        self.peek() == T::Other && self.text(self.tok()) == symbol
    }

    /// Whether the step ends here: at the end of its line or of the text, at a `;`, or at the `)` that closes the
    /// query in brackets it is part of.
    fn at_end(&self) -> bool {
        match self.peek() {
            T::Newline | T::Eof | T::Semicolon => true,
            T::RParen => self.brackets == 0 && self.queries > 0,
            _ => false,
        }
    }

    fn bump(&mut self) -> Token {
        let token = self.tok();
        if token.kind != T::Eof {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, kind: T) -> Option<Token> {
        self.at(kind).then(|| self.bump())
    }

    fn text(&self, token: Token) -> &'a str {
        &self.src[token.span.range()]
    }

    /// Where the token before the current one ends.
    fn previous_end(&self) -> usize {
        match self.pos {
            0 => self.tok().span.start as usize,
            pos => self.tokens[pos - 1].span.end as usize,
        }
    }

    /// Runs `read` with one more bracket open, so that line breaks do not end the step.
    fn inside<X>(&mut self, read: impl FnOnce(&mut Self) -> Parse<X>) -> Parse<X> {
        self.brackets += 1;
        let result = read(self);
        self.brackets -= 1;
        result
    }

    // ---- Problems

    /// Reports a problem that stops the step being read.
    fn report(&mut self, diagnostic: Diagnostic) -> Fail {
        self.failed_in = self.brackets;
        self.diagnostics.push(diagnostic);
        Fail
    }

    /// Reports a problem and reads on, as if what was meant had been written.
    fn note(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// A token, described for a message.
    fn describe(&self, token: Token) -> String {
        match token.kind {
            T::Eof => "the end of the text".to_string(),
            T::Str => "a string".to_string(),
            T::FString => "an f-string".to_string(),
            _ => {
                let text = self.text(token);
                if text.chars().count() > 30 {
                    format!("`{}…`", text.chars().take(30).collect::<String>())
                } else {
                    format!("`{text}`")
                }
            }
        }
    }

    /// Where the current token is, and what it is: for an "expected ..., found ..." message.
    fn found(&self) -> (Span, String) {
        let token = self.tok();
        match self.peek() {
            T::Newline => (
                Span::empty(self.previous_end()),
                "the end of the line".to_string(),
            ),
            T::Eof => (Span::empty(self.previous_end()), self.describe(token)),
            _ => (token.span, self.describe(token)),
        }
    }

    fn expected(&mut self, what: &str) -> Fail {
        let (span, found) = self.found();
        self.report(Diagnostic::error(
            span,
            format!("expected {what}, found {found}"),
        ))
    }

    fn expect(&mut self, kind: T, what: &str) -> Parse<Token> {
        match self.eat(kind) {
            Some(token) => Ok(token),
            None => Err(self.expected(what)),
        }
    }

    /// Whether a token can start a step, to know where one starts after a mistake.
    fn starts_step(&self, token: Token) -> bool {
        match token.kind {
            T::From | T::Del | T::Def | T::Import => true,
            T::Name => STEP_WORDS.contains(&self.text(token)),
            _ => false,
        }
    }

    /// Whether a token, met while a bracket is still open, shows that the text has moved on to the next step: it
    /// starts a line no further in than the step itself, and is not a closing bracket.
    fn moved_on(&self, token: Token) -> bool {
        token.kind == T::Eof
            || (token.line_start
                && token.col.compare(self.block) != Some(Ordering::Greater)
                && !matches!(token.kind, T::RParen | T::RBracket | T::RBrace | T::Comma))
    }

    /// Skips the rest of a step after a problem, to where the next step starts. `start` is where the step began: the
    /// parser always moves past it, so that it can't stop twice in one place.
    fn recover(&mut self, start: usize) {
        let mut depth = std::mem::take(&mut self.failed_in);
        while self.tok().kind != T::Eof {
            let token = self.tok();
            let line_at_step =
                token.line_start && token.col.compare(self.block) != Some(Ordering::Greater);
            if self.pos > start && line_at_step && (depth == 0 || self.moved_on(token)) {
                break;
            }
            match token.kind {
                T::LParen | T::LBracket | T::LBrace => depth += 1,
                T::RParen | T::RBracket | T::RBrace if depth > 0 => depth -= 1,
                T::RParen if self.queries > 0 => break,
                T::Semicolon if depth == 0 => {
                    self.bump();
                    break;
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// After a step or a definition: the line must end there, or a `;` lead to the next step.
    fn end_of_step(&mut self) {
        if self.at_end() {
            self.eat(T::Semicolon);
            return;
        }
        let token = self.tok();
        let mut problem = Diagnostic::error(
            token.span,
            format!(
                "expected the end of the step, found {}",
                self.describe(token)
            ),
        );
        if self.starts_step(token) {
            problem = problem
                .with_label("a new step starts here")
                .with_help("put each step on a line of its own, or separate steps with `;`");
        }
        self.report(problem);
        self.recover(self.pos);
    }

    /// Whether a line belongs with the lines at the current column, reporting one indented wrongly.
    fn line_belongs(&mut self, token: Token, end: End) -> bool {
        match token.col.compare(self.block) {
            Some(Ordering::Equal) => true,
            Some(Ordering::Greater) => {
                self.note(
                    Diagnostic::error(token.span, "unexpected indentation")
                        .with_label("this line is indented more than the one before it")
                        .with_help("only the lines of a block, after a `:`, are indented"),
                );
                true
            }
            Some(Ordering::Less) => match end {
                End::Block => false,
                End::Bracket(outer) if token.col.compare(outer) != Some(Ordering::Greater) => false,
                End::File | End::Bracket(_) => {
                    self.note(
                        Diagnostic::error(token.span, "unexpected indentation")
                            .with_label("this line is indented less than the lines before it"),
                    );
                    true
                }
            },
            None => {
                self.note(
                    Diagnostic::error(token.span, "this line's indentation mixes tabs and spaces")
                        .with_help("indent with spaces only, or with tabs only"),
                );
                true
            }
        }
    }

    // ---- Files

    fn file(&mut self) -> File {
        let mut file = File::default();
        let first = self.tok();
        if first.kind == T::Eof {
            return file;
        }
        // The first line sets the column, so a query indented inside a Python string reads as it is.
        self.block = first.col;
        file.docstring = self.docstring();
        loop {
            let token = self.tok();
            if token.kind == T::Eof {
                break;
            }
            if token.kind == T::Semicolon {
                self.bump();
                continue;
            }
            if token.line_start {
                self.line_belongs(token, End::File);
            }
            let start = self.pos;
            self.line_head = start;
            let next = self.nth(1);
            let read = match token.kind {
                T::Import => self
                    .import()
                    .map(|imports| file.items.extend(imports.into_iter().map(Item::Import))),
                T::From if self.is_from_import() => self
                    .import_from()
                    .map(|import| file.items.push(Item::Import(import))),
                T::Def => self.def().map(|def| file.items.push(Item::Def(def))),
                T::Name | T::QuotedName if next.kind == T::Assign && !next.line_start => self
                    .top_let()
                    .map(|value| file.items.push(Item::Let(value))),
                _ => {
                    // The query: its steps run to the end of the file.
                    file.query = Some(self.query(End::File));
                    break;
                }
            };
            match read {
                Ok(()) => self.end_of_step(),
                Err(Fail) => self.recover(start),
            }
        }
        file
    }

    /// The string alone on the first line of a file or a `def`, if there is one.
    fn docstring(&mut self) -> Option<Text> {
        let token = self.tok();
        let next = self.nth(1);
        if token.kind != T::Str
            || !(next.kind == T::Eof || next.line_start || next.kind == T::Semicolon)
        {
            return None;
        }
        self.bump();
        let mut parts = Vec::new();
        self.string_parts(token, &mut parts);
        self.eat(T::Semicolon);
        let value = parts
            .into_iter()
            .map(|part| match part {
                FPart::Text(text) => text,
                FPart::Value(_) => String::new(),
            })
            .collect();
        Some(Text {
            value,
            span: token.span,
        })
    }

    /// `name = value` before the query.
    fn top_let(&mut self) -> Parse<Let> {
        let name = self.first_name();
        let equals = self.bump();
        let value = self.value_after(equals.span, "a value, or a query in brackets, after `=`")?;
        Ok(Let {
            span: name.span.to(value.span),
            name,
            value,
        })
    }

    // ---- Imports

    /// Whether a `from` starts an import, `from shared import recent`, rather than a query.
    fn is_from_import(&self) -> bool {
        let mut n = 1;
        while self.nth(n).kind == T::Dot {
            n += 1;
        }
        while matches!(self.nth(n).kind, T::Name | T::QuotedName) {
            n += 1;
            if self.nth(n).kind != T::Dot {
                break;
            }
            n += 1;
        }
        let import = self.nth(n);
        import.kind == T::Import && !import.line_start
    }

    /// `import shared`, `import shared as s, other`.
    fn import(&mut self) -> Parse<Vec<Import>> {
        let keyword = self.bump();
        let mut imports = Vec::new();
        loop {
            let path = self.path("the name of a module")?;
            let alias = self.alias()?;
            let end = alias.as_ref().map_or(path.span, |alias| alias.span);
            let start = if imports.is_empty() {
                keyword.span
            } else {
                path.span
            };
            imports.push(Import {
                span: start.to(end),
                kind: ImportKind::Module { path, alias },
            });
            if self.eat(T::Comma).is_none() {
                return Ok(imports);
            }
        }
    }

    /// `from shared import recent, busy as b`, `from . import x`, `from shared import (a, b)`.
    fn import_from(&mut self) -> Parse<Import> {
        let keyword = self.bump();
        let mut level = 0;
        while self.eat(T::Dot).is_some() {
            level += 1;
        }
        let module = match self.peek() {
            T::Name | T::QuotedName => Some(self.path("the name of a module")?),
            _ => None,
        };
        self.expect(T::Import, "`import`")?;
        if self.at(T::Star) {
            let star = self.tok();
            return Err(self.report(
                Diagnostic::error(star.span, "`import *` is not supported")
                    .with_label("name what to import")
                    .with_help("for example: `from shared import recent, busy`"),
            ));
        }
        let (names, end) = match self.eat(T::LParen) {
            Some(open) => self.inside(|p| {
                let names = p.import_names(true)?;
                let close = p.close(open, T::RParen, "`,` or `)`")?;
                Ok((names, close.span))
            })?,
            None => {
                let names = self.import_names(false)?;
                let end = names.last().map_or(keyword.span, |last| {
                    last.alias
                        .as_ref()
                        .map_or(last.name.span, |alias| alias.span)
                });
                (names, end)
            }
        };
        Ok(Import {
            span: keyword.span.to(end),
            kind: ImportKind::Names {
                level,
                module,
                names,
            },
        })
    }

    fn import_names(&mut self, bracketed: bool) -> Parse<Vec<ImportName>> {
        let mut names = Vec::new();
        loop {
            if bracketed && self.at(T::RParen) {
                break;
            }
            let name = self.name("a name to import")?;
            let alias = self.alias()?;
            names.push(ImportName { name, alias });
            if self.eat(T::Comma).is_none() {
                break;
            }
        }
        if names.is_empty() {
            return Err(self.expected("a name to import"));
        }
        Ok(names)
    }

    /// `as name`, if it is there.
    fn alias(&mut self) -> Parse<Option<Name>> {
        match self.eat(T::As) {
            Some(_) => self.name("a name after `as`").map(Some),
            None => Ok(None),
        }
    }

    // ---- Definitions

    /// `def name(parameters):` and its body.
    fn def(&mut self) -> Parse<Def> {
        let keyword = self.bump();
        let name = self.name("a name after `def`")?;
        let open = self.expect(T::LParen, "`(` and the parameters")?;
        let (params, _) = self.inside(|p| p.params(open))?;
        let returns = match self.eat(T::Arrow) {
            Some(_) => Some(self.type_expr()?),
            None => None,
        };
        let Some(colon) = self.eat(T::Colon) else {
            return Err(self.expected("`:` at the end of the `def` line"));
        };
        let (docstring, body) = self.def_body(colon)?;
        let end = body
            .last()
            .map(|step| step.span)
            .or(docstring.as_ref().map(|text| text.span))
            .unwrap_or(colon.span);
        Ok(Def {
            name,
            params,
            returns,
            docstring,
            body,
            span: keyword.span.to(end),
        })
    }

    fn params(&mut self, open: Token) -> Parse<(Vec<Param>, Token)> {
        let mut params: Vec<Param> = Vec::new();
        loop {
            if self.at(T::RParen) {
                break;
            }
            if matches!(self.peek(), T::Star | T::DoubleStar | T::Slash) {
                let token = self.tok();
                return Err(self.report(Diagnostic::error(
                    token.span,
                    "`*args`, `**kwargs`, `*` and `/` are not supported in a `def`'s parameters",
                )));
            }
            let name = self.name("a parameter's name")?;
            let annotation = match self.eat(T::Colon) {
                Some(_) => Some(self.type_expr()?),
                None => None,
            };
            let default = match self.eat(T::Assign) {
                Some(equals) => Some(self.value_after(equals.span, "a value after `=`")?),
                None => None,
            };
            if default.is_none() && params.iter().any(|param| param.default.is_some()) {
                self.note(
                    Diagnostic::error(
                        name.span,
                        "a parameter without a default can't follow one with a default",
                    )
                    .with_help("put the parameters that have defaults last, as Python does"),
                );
            }
            let end = default
                .as_ref()
                .map(|value| value.span)
                .or(annotation.as_ref().map(|annotation| annotation.span))
                .unwrap_or(name.span);
            params.push(Param {
                span: name.span.to(end),
                name,
                annotation,
                default,
            });
            if self.eat(T::Comma).is_none() {
                break;
            }
        }
        let close = self.close(open, T::RParen, "`,` or `)`")?;
        Ok((params, close))
    }

    /// A type: `str`, `int`, `list[str]`, `None`.
    fn type_expr(&mut self) -> Parse<Type> {
        let name = if self.at(T::None) {
            let token = self.bump();
            Path {
                parts: vec![Name {
                    text: "None".to_string(),
                    span: token.span,
                }],
                span: token.span,
            }
        } else {
            self.path("a type, such as `str` or `int`")?
        };
        let mut span = name.span;
        let mut args = Vec::new();
        if let Some(open) = self.eat(T::LBracket) {
            let close = self.inside(|p| {
                loop {
                    args.push(p.type_expr()?);
                    if p.eat(T::Comma).is_none() {
                        break;
                    }
                }
                p.close(open, T::RBracket, "`,` or `]`")
            })?;
            span = span.to(close.span);
        }
        Ok(Type { name, args, span })
    }

    /// A `def`'s body: on the lines after the `:`, indented, or on the rest of its line.
    fn def_body(&mut self, colon: Token) -> Parse<(Option<Text>, Vec<Step>)> {
        let first = self.tok();
        if first.kind != T::Eof && !first.line_start {
            let steps = self.inline_steps();
            self.check_from(first, &steps, false);
            return Ok((None, steps));
        }
        if first.kind == T::Eof || first.col.compare(self.block) != Some(Ordering::Greater) {
            return Err(self.missing_block(
                colon,
                "the `def`'s steps",
                "where raised > now() - days(7)",
            ));
        }
        let outer = self.block;
        self.block = first.col;
        let docstring = self.docstring();
        let start = self.tok();
        let steps = self.steps(End::Block);
        self.block = outer;
        self.check_from(start, &steps, false);
        Ok((docstring, steps))
    }

    fn missing_block(&mut self, colon: Token, what: &str, example: &str) -> Fail {
        self.report(
            Diagnostic::error(
                colon.span,
                format!("expected {what} on the lines after this `:`, indented"),
            )
            .with_label("the block after this `:` is missing")
            .with_help(format!("for example:\n\n    {example}")),
        )
    }

    // ---- Queries and steps

    /// A query: its steps, from its `from`.
    fn query(&mut self, end: End) -> Query {
        let first = self.tok();
        let steps = self.steps(end);
        self.check_from(first, &steps, true);
        let span = match (steps.first(), steps.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            _ => first.span,
        };
        Query { steps, span }
    }

    /// Checks the `from` steps: the first step of a query, starting at `first`, is its one `from`. A first step that
    /// could not be read has had its problem reported already.
    fn check_from(&mut self, first: Token, steps: &[Step], query: bool) {
        let starts_with_from = first.kind == T::From;
        let first_read = steps
            .first()
            .is_some_and(|step| step.span.start == first.span.start);
        if query && first_read && !starts_with_from {
            self.note(
                Diagnostic::error(first.span, "a query starts with `from` and a table")
                    .with_label("this step has no rows to work on")
                    .with_help("for example: `from cases`"),
            );
        }
        for step in steps {
            if matches!(step.kind, StepKind::From(_)) && step.span.start != first.span.start {
                let problem = if starts_with_from {
                    Diagnostic::error(step.span, "a query has one `from`")
                        .with_label("this is a second `from`")
                        .with_help("to add another table's columns, use `join`")
                } else {
                    Diagnostic::error(
                        step.span,
                        "a `from` starts a query, so it goes on the query's first line",
                    )
                };
                self.note(problem);
            }
        }
    }

    /// The steps at the current column, to the end given.
    fn steps(&mut self, end: End) -> Vec<Step> {
        let mut steps = Vec::new();
        loop {
            let token = self.tok();
            if token.kind == T::Eof || (matches!(end, End::Bracket(_)) && token.kind == T::RParen) {
                break;
            }
            if token.kind == T::Semicolon {
                self.bump();
                continue;
            }
            if token.line_start && !self.line_belongs(token, end) {
                break;
            }
            let start = self.pos;
            self.line_head = start;
            match self.step() {
                Ok(step) => {
                    steps.push(step);
                    self.end_of_step();
                }
                Err(Fail) => self.recover(start),
            }
        }
        steps
    }

    /// The steps on the rest of a line, separated by `;`: `def recent(): where raised > now() - days(7)`.
    fn inline_steps(&mut self) -> Vec<Step> {
        let mut steps = Vec::new();
        loop {
            let start = self.pos;
            self.line_head = start;
            match self.step() {
                Ok(step) => steps.push(step),
                Err(Fail) => {
                    self.recover(start);
                    break;
                }
            }
            if self.eat(T::Semicolon).is_none() || self.at_end() {
                break;
            }
        }
        steps
    }

    fn step(&mut self) -> Parse<Step> {
        let token = self.tok();
        let next = self.nth(1);
        let assigns = matches!(next.kind, T::Assign | T::AugAssign) && !next.line_start;
        match token.kind {
            T::From => self.source_step(),
            T::Del => self.del_step(),
            T::Pass => {
                self.bump();
                Ok(Step {
                    kind: StepKind::Pass,
                    span: token.span,
                })
            }
            T::Name | T::QuotedName if assigns => self.column_step(),
            T::Name => match self.text(token) {
                "where" => self.where_step(),
                "select" => self.select_step(),
                "sort" => self.sort_step(),
                "take" => self.take_step(false),
                "skip" => self.take_step(true),
                "distinct" => self.distinct_step(),
                "group" => self.group_step(),
                "summarize" => self.summarize_step(),
                "join" => self.join_step(None),
                "left" if next.kind == T::Name && self.text(next) == "join" && !next.line_start => {
                    let left = self.bump();
                    self.join_step(Some(left))
                }
                "left" => Err(self.report(
                    Diagnostic::error(token.span, "expected `join` after `left`")
                        .with_help("for example: `left join notes on notes.case_id == id`"),
                )),
                _ if matches!(next.kind, T::LParen | T::Dot) && !next.line_start => {
                    self.call_step()
                }
                _ => Err(self.unknown_step(token)),
            },
            T::Import => Err(self.report(
                Diagnostic::error(
                    token.span,
                    "imports go at the top of the file, before the query",
                )
                .with_label("move this line up"),
            )),
            T::Def => Err(self.report(
                Diagnostic::error(token.span, "a `def` goes before the query")
                    .with_label("move the `def` above the query's `from`"),
            )),
            T::Str | T::FString => Err(self.report(
                Diagnostic::error(token.span, "a string alone is not a step")
                    .with_help("a docstring goes on the first line of the file or of a `def`"),
            )),
            T::If | T::Else | T::Reserved => {
                let word = self.text(token);
                Err(self.report(
                    Diagnostic::error(token.span, format!("`{word}` is not a ButtercupQL step"))
                        .with_help(reserved_help(word)),
                ))
            }
            _ => Err(self.expected("a step, such as `where` or `sort`")),
        }
    }

    /// A line that starts with a word that is no step: a typing mistake, SQL, or a condition without its `where`.
    fn unknown_step(&mut self, token: Token) -> Fail {
        let word = self.text(token);
        let lower = word.to_lowercase();
        let next = self.nth(1);
        let next_word = if next.line_start { "" } else { self.text(next) };
        let mut problem = Diagnostic::error(token.span, format!("there is no step `{word}`"));
        if lower != word && (STEP_WORDS.contains(&lower.as_str()) || lower == "del") {
            problem = problem
                .with_label(format!("write it in lowercase: `{lower}`"))
                .with_help("ButtercupQL's words are lowercase, as Python's are");
        } else if let Some(&(_, step, example)) =
            OTHER_STEPS.iter().find(|(other, _, _)| *other == lower)
        {
            problem = problem
                .with_label(format!("ButtercupQL calls this `{step}`"))
                .with_help(format!("for example: `{example}`"));
            if lower == "having" {
                problem = problem
                    .with_help("after `group by`, a `where` keeps the groups whose totals pass");
            }
        } else if matches!(lower.as_str(), "right" | "full" | "cross" | "outer")
            && next_word == "join"
        {
            problem = problem.with_help(
                "ButtercupQL has `join`, and `left join` to keep the rows that have no match",
            );
        } else if let Some(step) = suggest::closest(word, STEP_WORDS.into_iter().chain(["pass"])) {
            let step = match step {
                "group" => "group by",
                "left" => "left join",
                step => step,
            };
            problem = problem.with_label(format!("did you mean `{step}`?"));
        } else if matches!(
            next.kind,
            T::EqEq | T::NotEq | T::Lt | T::LtEq | T::Gt | T::GtEq | T::In | T::Not | T::Is
        ) && !next.line_start
        {
            problem = problem.with_help(format!(
                "to keep the rows where this is true, start the line with `where`: `where {word} ...`"
            ));
        } else {
            problem = problem.with_help(
                "a step starts with a word such as `where`, `select`, `sort`, `take` or `group by`, or names a column: `total = price * quantity`",
            );
        }
        self.report(problem)
    }

    fn source_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        let source = self.source("a table after `from`")?;
        Ok(Step {
            span: keyword.span.to(source.span),
            kind: StepKind::From(source),
        })
    }

    /// What a `from` or a `join` reads: `cases`, `analytics.daily`, `cases_for("A")`, `(from ...)`, with `as name`.
    fn source(&mut self, what: &str) -> Parse<Source> {
        let value = match self.peek() {
            T::LParen if self.nth(1).kind == T::From => self.bracketed_query()?,
            T::Name | T::QuotedName => self.primary()?,
            T::Str | T::FString => {
                let token = self.tok();
                return Err(self.report(
                    Diagnostic::error(token.span, "a table's name is not a string")
                        .with_help("write the name as it is, as in `from cases`, or in backticks if it is not a plain word: `from `order lines``"),
                ));
            }
            _ => return Err(self.expected(what)),
        };
        if !is_source(&value) {
            return Err(self.report(
                Diagnostic::error(value.span, format!("expected {what}"))
                    .with_label("this is not a table, a named query or a call of a query `def`"),
            ));
        }
        let alias = self.alias()?;
        let span = alias
            .as_ref()
            .map_or(value.span, |alias| value.span.to(alias.span));
        Ok(Source { value, alias, span })
    }

    /// A query in brackets: `(from cases; where risk > 90)`, or its steps on lines of their own.
    fn bracketed_query(&mut self) -> Parse<Expr> {
        let open = self.bump();
        let (brackets, outer) = (self.brackets, self.block);
        self.brackets = 0;
        self.queries += 1;
        self.block = self.tok().col;
        let query = self.query(End::Bracket(outer));
        self.queries -= 1;
        self.block = outer;
        self.brackets = brackets;
        let end = match self.tok().kind {
            T::RParen => self.bump().span.end as usize,
            _ => {
                self.note(
                    Diagnostic::error(open.span, "this `(` is never closed")
                        .with_label("the query in brackets starts here")
                        .with_help("close it with `)` after its last step"),
                );
                self.previous_end()
            }
        };
        let span = Span::new(open.span.start as usize, end);
        Ok(Expr {
            span,
            kind: ExprKind::Query(Box::new(query)),
        })
    }

    fn where_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        let condition = self.value_after(
            keyword.span,
            "a condition after `where`, as in `where risk >= 90`",
        )?;
        Ok(Step {
            span: keyword.span.to(condition.span),
            kind: StepKind::Where(condition),
        })
    }

    /// `name = value`, or `name += value`.
    fn column_step(&mut self) -> Parse<Step> {
        let name = self.first_name();
        let operator = self.bump();
        let value = self.value_after(
            operator.span,
            &format!("a value after `{}`", self.text(operator)),
        )?;
        let value = if operator.kind == T::AugAssign {
            let op = match self.text(operator).trim_end_matches('=') {
                "+" => BinaryOp::Add,
                "-" => BinaryOp::Sub,
                "*" => BinaryOp::Mul,
                "/" => BinaryOp::Div,
                "//" => BinaryOp::FloorDiv,
                "%" => BinaryOp::Mod,
                _ => BinaryOp::Pow,
            };
            let column = Expr {
                kind: ExprKind::Name(name.text.clone()),
                span: name.span,
            };
            Expr {
                span: name.span.to(value.span),
                kind: ExprKind::Binary(op, Box::new(column), Box::new(value)),
            }
        } else {
            value
        };
        Ok(Step {
            span: name.span.to(value.span),
            kind: StepKind::Column { name, value },
        })
    }

    fn select_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        let (fields, end) = self.fields("a column to select")?;
        Ok(Step {
            span: keyword.span.to(end),
            kind: StepKind::Select(fields),
        })
    }

    fn del_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        let mut columns = Vec::new();
        loop {
            if self.at_end() {
                return Err(self.expected("a column to drop"));
            }
            let column = self.primary()?;
            if !is_path(&column) {
                return Err(self.report(
                    Diagnostic::error(column.span, "`del` takes the names of columns")
                        .with_help("for example: `del notes, raw_text`"),
                ));
            }
            columns.push(column);
            if self.eat(T::Comma).is_none() || self.at_end() {
                break;
            }
        }
        let end = columns.last().map_or(keyword.span, |column| column.span);
        Ok(Step {
            span: keyword.span.to(end),
            kind: StepKind::Del(columns),
        })
    }

    fn sort_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        if self.at_word("by")
            && !matches!(
                self.peek_nth(1),
                T::Comma | T::Newline | T::Eof | T::Semicolon
            )
        {
            let by = self.tok();
            return Err(self.report(
                Diagnostic::error(
                    by.span,
                    "`sort` takes its columns straight after it, without `by`",
                )
                .with_label("remove `by`")
                .with_help("for example: `sort -risk, raised`, where `-` sorts high to low"),
            ));
        }
        let mut keys = Vec::new();
        let mut reverse = None;
        let mut end: Span;
        loop {
            if self.at_word("reverse") && self.peek_nth(1) == T::Assign {
                self.bump();
                let equals = self.bump();
                let value = self.value_after(equals.span, "`True` or `False` after `reverse=`")?;
                end = value.span;
                reverse = Some(value);
                if self.at(T::Comma) {
                    let comma = self.tok();
                    return Err(self.report(Diagnostic::error(
                        comma.span,
                        "`reverse=` goes after the columns",
                    )));
                }
                break;
            }
            if self.at_end() {
                return Err(self.expected("a column to sort by"));
            }
            let value = self.expr()?;
            let span = value.span;
            let mut key = match value.kind {
                ExprKind::Unary(UnaryOp::Neg, inner) => SortKey {
                    value: *inner,
                    descending: true,
                    span,
                },
                kind => SortKey {
                    value: Expr { kind, span },
                    descending: false,
                    span,
                },
            };
            end = span;
            let word = if self.peek() == T::Name {
                self.text(self.tok()).to_lowercase()
            } else {
                String::new()
            };
            if word == "desc" || word == "asc" {
                let order = self.bump();
                let column = &self.src[key.value.span.range()];
                let fix = if word == "desc" {
                    format!("write `-{column}` to sort high to low")
                } else {
                    format!("write `{column}`: low to high is the order already")
                };
                self.note(
                    Diagnostic::error(
                        order.span,
                        format!("there is no `{}` in ButtercupQL", self.text(order)),
                    )
                    .with_label(fix),
                );
                key.descending = word == "desc";
                end = order.span;
            }
            keys.push(key);
            if self.eat(T::Comma).is_none() {
                break;
            }
        }
        Ok(Step {
            span: keyword.span.to(end),
            kind: StepKind::Sort { keys, reverse },
        })
    }

    fn take_step(&mut self, skip: bool) -> Parse<Step> {
        let keyword = self.bump();
        let what = if skip {
            "how many rows to skip, as in `skip 40`"
        } else {
            "how many rows to take, as in `take 20`"
        };
        let count = self.value_after(keyword.span, what)?;
        Ok(Step {
            span: keyword.span.to(count.span),
            kind: if skip {
                StepKind::Skip(count)
            } else {
                StepKind::Take(count)
            },
        })
    }

    fn distinct_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        if !self.at_end() {
            let token = self.tok();
            return Err(self.report(
                Diagnostic::error(token.span, "`distinct` takes no columns")
                    .with_label("`distinct` drops repeated rows, comparing every column")
                    .with_help("to keep one row for each value of a column, use `group by`"),
            ));
        }
        Ok(Step {
            span: keyword.span,
            kind: StepKind::Distinct,
        })
    }

    fn group_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        if !self.at_word("by") {
            let (span, found) = self.found();
            return Err(self.report(
                Diagnostic::error(span, format!("expected `by` after `group`, found {found}"))
                    .with_help("for example: `group by operator:`"),
            ));
        }
        self.bump();
        let (by, mut end) = self.fields("a column to group by")?;
        let mut totals = Vec::new();
        if let Some(colon) = self.eat(T::Colon) {
            totals = self.block_of_totals(colon)?;
            end = totals.last().map_or(colon.span, |total| total.span);
        }
        Ok(Step {
            span: keyword.span.to(end),
            kind: StepKind::Group { by, totals },
        })
    }

    fn summarize_step(&mut self) -> Parse<Step> {
        let keyword = self.bump();
        let Some(colon) = self.eat(T::Colon) else {
            let (span, found) = self.found();
            return Err(self.report(
                Diagnostic::error(span, format!("expected `:` after `summarize`, found {found}"))
                    .with_help("the totals go on the lines after it, indented:\n\n    summarize:\n        n = count()"),
            ));
        };
        let totals = self.block_of_totals(colon)?;
        let end = totals.last().map_or(colon.span, |total| total.span);
        Ok(Step {
            span: keyword.span.to(end),
            kind: StepKind::Summarize(totals),
        })
    }

    /// The totals after a `:`: on the lines after it, indented, or on the rest of its line.
    fn block_of_totals(&mut self, colon: Token) -> Parse<Vec<Field>> {
        let first = self.tok();
        let mut totals = Vec::new();
        if first.kind != T::Eof && !first.line_start {
            loop {
                totals.push(self.total()?);
                if self.eat(T::Semicolon).is_none() || self.at_end() {
                    return Ok(totals);
                }
            }
        }
        if first.kind == T::Eof || first.col.compare(self.block) != Some(Ordering::Greater) {
            return Err(self.missing_block(colon, "the totals", "n = count()"));
        }
        let outer = self.block;
        self.block = first.col;
        loop {
            let token = self.tok();
            if token.kind == T::Eof || (token.kind == T::RParen && self.queries > 0) {
                break;
            }
            if token.kind == T::Semicolon {
                self.bump();
                continue;
            }
            if token.line_start && !self.line_belongs(token, End::Block) {
                break;
            }
            let start = self.pos;
            self.line_head = start;
            match self.total() {
                Ok(total) => {
                    totals.push(total);
                    self.end_of_step();
                }
                Err(Fail) => self.recover(start),
            }
        }
        self.block = outer;
        Ok(totals)
    }

    /// A line of a block of totals: `name = value`.
    fn total(&mut self) -> Parse<Field> {
        let token = self.tok();
        let next = self.nth(1);
        if matches!(token.kind, T::Name | T::QuotedName)
            && next.kind == T::Assign
            && !next.line_start
        {
            let name = self.first_name();
            let equals = self.bump();
            let value = self.value_after(equals.span, "a total after `=`, such as `count()`")?;
            return Ok(Field {
                span: name.span.to(value.span),
                name: Some(name),
                value,
            });
        }
        if self.starts_step(token) && !matches!(next.kind, T::LParen | T::Dot) {
            let word = self.text(token);
            let problem = if word == "where" {
                Diagnostic::error(token.span, "a `where` can't go inside a block of totals")
                    .with_help("to filter the rows, put the `where` before the step; to filter the totals, put it after the block, at the step's indentation")
                    .with_help("to count only some rows, give the total a condition: `big = count(where=amount > 1_000_000)`")
            } else {
                Diagnostic::error(
                    token.span,
                    format!("a `{word}` can't go inside a block of totals"),
                )
                .with_help(
                    "put it after the block, at the indentation of the step the block belongs to",
                )
            };
            return Err(self.report(problem));
        }
        let value = self.expr()?;
        Err(self.report(
            Diagnostic::error(value.span, "this total needs a name")
                .with_label("give it a name, as in `n = count()`")
                .with_help("each line of the block names a total"),
        ))
    }

    fn join_step(&mut self, left: Option<Token>) -> Parse<Step> {
        let keyword = self.bump();
        let start = left.map_or(keyword.span, |left| left.span);
        let source = self.source("a table to join")?;
        if !self.at_word("on") {
            let (span, found) = self.found();
            return Err(self.report(
                Diagnostic::error(
                    span,
                    format!("expected `on` and the condition that pairs the rows, found {found}"),
                )
                .with_help("for example: `join accounts on account_id == accounts.id`"),
            ));
        }
        let on = self.bump();
        let condition = self.value_after(on.span, "a condition after `on`")?;
        Ok(Step {
            span: start.to(condition.span),
            kind: StepKind::Join {
                left: left.is_some(),
                source,
                on: condition,
            },
        })
    }

    /// `recent(30)` or `shared.recent(30)`: the steps of a `def`, put in place.
    fn call_step(&mut self) -> Parse<Step> {
        let call = self.primary()?;
        if !matches!(call.kind, ExprKind::Call(..)) {
            let problem = Diagnostic::error(call.span, "this is not a step")
                .with_help("to use the steps of a `def`, call it with brackets: `recent()`");
            return Err(self.report(problem));
        }
        Ok(Step {
            span: call.span,
            kind: StepKind::Call(call),
        })
    }

    /// The columns of a `select` or a `group by`: `a, b, total = x + y`, on one line or in brackets. Returns them
    /// and where they end.
    fn fields(&mut self, what: &str) -> Parse<(Vec<Field>, Span)> {
        if self.at(T::LParen) && self.peek_nth(1) != T::From {
            let open = self.bump();
            return self.inside(|p| {
                let fields = p.field_list(what, true)?;
                let close = p.close(open, T::RParen, "`,` or `)`")?;
                Ok((fields, close.span))
            });
        }
        let fields = self.field_list(what, false)?;
        let end = fields
            .last()
            .map_or(Span::empty(self.previous_end()), |field| field.span);
        Ok((fields, end))
    }

    fn field_list(&mut self, what: &str, bracketed: bool) -> Parse<Vec<Field>> {
        let mut fields = Vec::new();
        loop {
            if bracketed && self.at(T::RParen) {
                break;
            }
            fields.push(self.field(what)?);
            if self.eat(T::Comma).is_none() {
                break;
            }
            if !bracketed && (self.at_end() || self.at(T::Colon)) {
                break;
            }
        }
        if fields.is_empty() {
            return Err(self.expected(what));
        }
        Ok(fields)
    }

    fn field(&mut self, what: &str) -> Parse<Field> {
        if matches!(self.peek(), T::Name | T::QuotedName) && self.peek_nth(1) == T::Assign {
            let name = self.name(what)?;
            let equals = self.bump();
            let value = self.value_after(equals.span, "a value after `=`")?;
            return Ok(Field {
                span: name.span.to(value.span),
                name: Some(name),
                value,
            });
        }
        if self.at_end() || self.at(T::Comma) || self.at(T::Colon) {
            return Err(self.expected(what));
        }
        let value = self.expr()?;
        if self.at(T::As) {
            let keyword = self.bump();
            let alias = self.name("a name after `as`")?;
            let fix = format!("write `{} = {}`", alias.text, &self.src[value.span.range()]);
            self.note(
                Diagnostic::error(
                    keyword.span.to(alias.span),
                    "name a column with `=`, not `as`",
                )
                .with_label(fix),
            );
            return Ok(Field {
                span: value.span.to(alias.span),
                name: Some(alias),
                value,
            });
        }
        Ok(Field {
            span: value.span,
            name: None,
            value,
        })
    }

    // ---- Names

    fn name(&mut self, what: &str) -> Parse<Name> {
        let token = self.tok();
        match self.peek() {
            T::Name => {
                self.bump();
                Ok(Name {
                    text: self.text(token).to_string(),
                    span: token.span,
                })
            }
            T::QuotedName => {
                self.bump();
                Ok(Name {
                    text: self.quoted_text(token),
                    span: token.span,
                })
            }
            T::And
            | T::As
            | T::Def
            | T::Del
            | T::Else
            | T::False
            | T::From
            | T::If
            | T::Import
            | T::In
            | T::Is
            | T::None
            | T::Not
            | T::Or
            | T::Pass
            | T::True
            | T::Reserved => {
                let word = self.text(token);
                Err(self.report(
                    Diagnostic::error(
                        token.span,
                        format!("expected {what}, found the keyword `{word}`"),
                    )
                    .with_help(format!(
                        "to use `{word}` as a name, put it in backticks: `` `{word}` ``"
                    )),
                ))
            }
            _ => Err(self.expected(what)),
        }
    }

    /// The name that starts a line, which the caller has checked is a name.
    fn first_name(&mut self) -> Name {
        let token = self.bump();
        let text = match token.kind {
            T::QuotedName => self.quoted_text(token),
            _ => self.text(token).to_string(),
        };
        Name {
            text,
            span: token.span,
        }
    }

    /// The text of a name in backticks.
    fn quoted_text(&self, token: Token) -> String {
        let text = self.text(token);
        let text = text.strip_prefix('`').unwrap_or(text);
        let text = text.strip_suffix('`').unwrap_or(text);
        text.replace("``", "`")
    }

    /// A dotted name: `shared.recent`.
    fn path(&mut self, what: &str) -> Parse<Path> {
        let mut parts = vec![self.name(what)?];
        while self.eat(T::Dot).is_some() {
            parts.push(self.name("a name after `.`")?);
        }
        let span = parts[0].span.to(parts[parts.len() - 1].span);
        Ok(Path { parts, span })
    }

    // ---- Expressions, with Python's order of operations

    /// A value after a keyword or an operator, with a clear message when there is none.
    fn value_after(&mut self, after: Span, what: &str) -> Parse<Expr> {
        if self.at_end() {
            return Err(self.report(Diagnostic::error(
                Span::empty(after.end as usize),
                format!("expected {what}"),
            )));
        }
        self.expr()
    }

    /// An expression: `a if condition else b`, or anything simpler.
    fn expr(&mut self) -> Parse<Expr> {
        let then = self.disjunction()?;
        if self.eat(T::If).is_none() {
            return Ok(then);
        }
        let test = self.disjunction()?;
        if self.eat(T::Else).is_none() {
            return Err(self
                .expected("`else` and a value, as in `\"urgent\" if risk >= 90 else \"normal\"`"));
        }
        let otherwise = self.expr()?;
        Ok(Expr {
            span: then.span.to(otherwise.span),
            kind: ExprKind::IfElse {
                test: Box::new(test),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            },
        })
    }

    fn disjunction(&mut self) -> Parse<Expr> {
        let mut left = self.conjunction()?;
        loop {
            if self.eat(T::Or).is_none() {
                if !(self.at_other("|") || self.at_other("||")) {
                    return Ok(left);
                }
                let token = self.bump();
                let mut problem = Diagnostic::error(token.span, "use `or`").with_label(format!(
                    "`{}` is not how ButtercupQL writes “or”",
                    self.text(token)
                ));
                if self.text(token) == "||" {
                    problem =
                        problem.with_help("to join text, use `+` or an f-string, as in Python");
                }
                self.note(problem);
            }
            let right = self.conjunction()?;
            left = binary(BinaryOp::Or, left, right);
        }
    }

    fn conjunction(&mut self) -> Parse<Expr> {
        let mut left = self.inversion()?;
        loop {
            if self.eat(T::And).is_none() {
                if !(self.at_other("&") || self.at_other("&&")) {
                    return Ok(left);
                }
                let token = self.bump();
                self.note(
                    Diagnostic::error(token.span, "use `and`").with_label(format!(
                        "`{}` is not how ButtercupQL writes “and”",
                        self.text(token)
                    )),
                );
            }
            let right = self.inversion()?;
            left = binary(BinaryOp::And, left, right);
        }
    }

    fn inversion(&mut self) -> Parse<Expr> {
        let token = self.tok();
        let wrong = self.at_other("!") || self.at_other("~");
        if !(self.at(T::Not) || wrong) {
            return self.comparison();
        }
        self.bump();
        if wrong {
            self.note(
                Diagnostic::error(token.span, "use `not`").with_label(format!(
                    "`{}` is not how ButtercupQL writes “not”",
                    self.text(token)
                )),
            );
        }
        let operand = self.inversion()?;
        Ok(Expr {
            span: token.span.to(operand.span),
            kind: ExprKind::Unary(UnaryOp::Not, Box::new(operand)),
        })
    }

    fn comparison(&mut self) -> Parse<Expr> {
        let first = self.sum()?;
        let mut rest = Vec::new();
        loop {
            let token = self.tok();
            let op = match self.peek() {
                T::EqEq => CompareOp::Eq,
                T::NotEq => CompareOp::NotEq,
                T::Lt => CompareOp::Lt,
                T::LtEq => CompareOp::LtEq,
                T::Gt => CompareOp::Gt,
                T::GtEq => CompareOp::GtEq,
                T::In => CompareOp::In,
                T::Not if self.peek_nth(1) == T::In => {
                    self.bump();
                    CompareOp::NotIn
                }
                T::Is if self.peek_nth(1) == T::Not => {
                    self.bump();
                    CompareOp::IsNot
                }
                T::Is => CompareOp::Is,
                T::Assign => {
                    self.note(
                        Diagnostic::error(token.span, "use `==` to compare")
                            .with_label("`=` gives a column a value; `==` compares two values"),
                    );
                    CompareOp::Eq
                }
                T::Other if self.text(token) == "<>" => {
                    self.note(Diagnostic::error(token.span, "use `!=` for “not equal to”"));
                    CompareOp::NotEq
                }
                _ => break,
            };
            let last = self.bump();
            if self.at_end() {
                return Err(self.report(Diagnostic::error(
                    Span::empty(last.span.end as usize),
                    format!("expected a value after `{}`", op.symbol()),
                )));
            }
            rest.push((op, self.sum()?));
        }
        if rest.is_empty() {
            return Ok(first);
        }
        let span = first.span.to(rest[rest.len() - 1].1.span);
        Ok(Expr {
            span,
            kind: ExprKind::Compare(Box::new(first), rest),
        })
    }

    fn sum(&mut self) -> Parse<Expr> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek() {
                T::Plus => BinaryOp::Add,
                T::Minus => BinaryOp::Sub,
                _ => return Ok(left),
            };
            let token = self.bump();
            let right = self.operand(token)?;
            let right = self.term_from(right)?;
            left = binary(op, left, right);
        }
    }

    fn term(&mut self) -> Parse<Expr> {
        let first = self.factor()?;
        self.term_from(first)
    }

    /// The rest of a term whose first factor has been read.
    fn term_from(&mut self, mut left: Expr) -> Parse<Expr> {
        loop {
            let op = match self.peek() {
                T::Star => BinaryOp::Mul,
                T::Slash => BinaryOp::Div,
                T::DoubleSlash => BinaryOp::FloorDiv,
                T::Percent => BinaryOp::Mod,
                _ => return Ok(left),
            };
            let token = self.bump();
            let right = self.operand(token)?;
            left = binary(op, left, right);
        }
    }

    /// The factor after an operator, with a clear message when the line ends first.
    fn operand(&mut self, operator: Token) -> Parse<Expr> {
        if self.at_end() {
            return Err(self.report(Diagnostic::error(
                Span::empty(operator.span.end as usize),
                format!("expected a value after `{}`", self.text(operator)),
            )));
        }
        self.factor()
    }

    fn factor(&mut self) -> Parse<Expr> {
        let token = self.tok();
        let op = match self.peek() {
            T::Minus => UnaryOp::Neg,
            T::Plus => UnaryOp::Pos,
            _ => return self.power(),
        };
        self.bump();
        let operand = self.operand(token)?;
        Ok(Expr {
            span: token.span.to(operand.span),
            kind: ExprKind::Unary(op, Box::new(operand)),
        })
    }

    fn power(&mut self) -> Parse<Expr> {
        let base = self.primary()?;
        match self.eat(T::DoubleStar) {
            Some(token) => {
                let exponent = self.operand(token)?;
                Ok(binary(BinaryOp::Pow, base, exponent))
            }
            None => Ok(base),
        }
    }

    /// A value and what follows it: `.name`, and calls.
    fn primary(&mut self) -> Parse<Expr> {
        let mut expr = self.atom()?;
        loop {
            match self.peek() {
                T::Dot => {
                    self.bump();
                    let name = self.name("a name after `.`")?;
                    expr = Expr {
                        span: expr.span.to(name.span),
                        kind: ExprKind::Attribute(Box::new(expr), name),
                    };
                }
                T::LParen => {
                    let open = self.bump();
                    let (args, close) = self.inside(|p| p.args(open))?;
                    expr = Expr {
                        span: expr.span.to(close.span),
                        kind: ExprKind::Call(Box::new(expr), args),
                    };
                }
                T::LBracket => {
                    let token = self.tok();
                    return Err(self.report(Diagnostic::error(
                        token.span,
                        "subscripts such as `name[0]` are not supported yet",
                    )));
                }
                _ => return Ok(expr),
            }
        }
    }

    fn args(&mut self, open: Token) -> Parse<(Vec<Arg>, Token)> {
        let mut args: Vec<Arg> = Vec::new();
        loop {
            if self.at(T::RParen) {
                break;
            }
            if matches!(self.peek(), T::Star | T::DoubleStar) {
                let token = self.tok();
                return Err(self.report(Diagnostic::error(
                    token.span,
                    "`*` and `**` arguments are not supported",
                )));
            }
            if matches!(self.peek(), T::Name | T::QuotedName) && self.peek_nth(1) == T::Assign {
                let name = self.name("an argument's name")?;
                let equals = self.bump();
                let value = self.value_after(equals.span, "a value after `=`")?;
                args.push(Arg {
                    name: Some(name),
                    value,
                });
            } else {
                let value = self.expr()?;
                if args.iter().any(|arg| arg.name.is_some()) {
                    self.note(
                        Diagnostic::error(
                            value.span,
                            "an argument without a name can't follow one with a name",
                        )
                        .with_help("put the arguments given by name last, as Python does"),
                    );
                }
                args.push(Arg { name: None, value });
            }
            if self.eat(T::Comma).is_none() {
                break;
            }
        }
        let close = self.close(open, T::RParen, "`,` or `)`")?;
        Ok((args, close))
    }

    /// The closing bracket of `open`. When it is missing, says that the bracket is never closed if the text has
    /// moved on to the next step, and what was expected otherwise.
    fn close(&mut self, open: Token, kind: T, expected: &str) -> Parse<Token> {
        if let Some(token) = self.eat(kind) {
            return Ok(token);
        }
        let token = self.tok();
        let bracket = self.text(open);
        if self.moved_on(token) {
            let closing = match kind {
                T::RBracket => "]",
                T::RBrace => "}",
                _ => ")",
            };
            return Err(self.report(
                Diagnostic::error(open.span, format!("this `{bracket}` is never closed"))
                    .with_label(format!("close it with `{closing}`")),
            ));
        }
        let (span, found) = self.found();
        Err(self.report(
            Diagnostic::error(span, format!("expected {expected}, found {found}"))
                .with_related(open.span, format!("the `{bracket}` opens here")),
        ))
    }

    fn atom(&mut self) -> Parse<Expr> {
        let token = self.tok();
        let kind = match self.peek() {
            T::Name => ExprKind::Name(self.text(token).to_string()),
            T::QuotedName => ExprKind::Name(self.quoted_text(token)),
            T::Int | T::Float => ExprKind::Number(self.number(token)),
            T::True => ExprKind::Bool(true),
            T::False => ExprKind::Bool(false),
            T::None => ExprKind::None,
            T::Str | T::FString => return self.strings(),
            T::LParen => return self.paren(),
            T::LBracket => return self.list(),
            T::LBrace => return self.set(),
            T::From => {
                return Err(self.report(
                    Diagnostic::error(token.span, "a query used as a value goes in brackets")
                        .with_label("put `(` before `from`, and `)` after the query's last step"),
                ));
            }
            T::Reserved => {
                let word = self.text(token);
                return Err(self.report(
                    Diagnostic::error(
                        token.span,
                        format!("`{word}` is a Python keyword that ButtercupQL does not use"),
                    )
                    .with_help(reserved_help(word)),
                ));
            }
            T::Other if matches!(self.text(token), "$" | "?" | "@") => {
                return Err(self.report(
                    Diagnostic::error(token.span, "ButtercupQL writes the SQL parameters itself")
                        .with_help("put the value in the query, as in `status == \"open\"`, or make it a parameter of a `def`"),
                ));
            }
            T::Other if self.text(token) == ":=" => {
                return Err(self.report(Diagnostic::error(token.span, "`:=` is not supported")));
            }
            _ => {
                let previous = self.pos.checked_sub(1).map(|at| self.tokens[at]);
                let after = match previous {
                    Some(previous) if is_operator(previous.kind) => {
                        format!(" after `{}`", self.text(previous))
                    }
                    _ => String::new(),
                };
                return Err(self.expected(&format!("a value{after}")));
            }
        };
        self.bump();
        Ok(Expr {
            kind,
            span: token.span,
        })
    }

    /// `(value)`, a tuple `(a, b)`, or a query in brackets.
    fn paren(&mut self) -> Parse<Expr> {
        if self.nth(1).kind == T::From {
            return self.bracketed_query();
        }
        let open = self.bump();
        self.inside(|p| {
            if let Some(close) = p.eat(T::RParen) {
                return Ok(Expr {
                    kind: ExprKind::Tuple(Vec::new()),
                    span: open.span.to(close.span),
                });
            }
            let first = p.expr()?;
            if !p.at(T::Comma) {
                p.no_comprehension(open)?;
                let close = p.close(open, T::RParen, "`)`")?;
                return Ok(Expr {
                    kind: first.kind,
                    span: open.span.to(close.span),
                });
            }
            let mut items = vec![first];
            while p.eat(T::Comma).is_some() {
                if p.at(T::RParen) {
                    break;
                }
                items.push(p.expr()?);
            }
            let close = p.close(open, T::RParen, "`,` or `)`")?;
            Ok(Expr {
                kind: ExprKind::Tuple(items),
                span: open.span.to(close.span),
            })
        })
    }

    /// `[a, b]`.
    fn list(&mut self) -> Parse<Expr> {
        let open = self.bump();
        self.inside(|p| {
            let (items, close) = p.items(open, T::RBracket)?;
            Ok(Expr {
                kind: ExprKind::List(items),
                span: open.span.to(close.span),
            })
        })
    }

    /// `{a, b}`.
    fn set(&mut self) -> Parse<Expr> {
        let open = self.bump();
        self.inside(|p| {
            if let Some(close) = p.eat(T::RBrace) {
                return Err(p.report(Diagnostic::error(
                    open.span.to(close.span),
                    "`{}` is an empty dictionary in Python, and dictionaries are not supported",
                )));
            }
            let (items, close) = p.items(open, T::RBrace)?;
            Ok(Expr {
                kind: ExprKind::Set(items),
                span: open.span.to(close.span),
            })
        })
    }

    /// The values of a list or a set, and its closing bracket.
    fn items(&mut self, open: Token, closing: T) -> Parse<(Vec<Expr>, Token)> {
        let mut items = Vec::new();
        loop {
            if self.at(closing) {
                break;
            }
            items.push(self.expr()?);
            self.no_comprehension(open)?;
            if closing == T::RBrace && self.at(T::Colon) {
                let token = self.tok();
                return Err(self.report(Diagnostic::error(
                    token.span,
                    "dictionaries are not supported",
                )));
            }
            if self.eat(T::Comma).is_none() {
                break;
            }
        }
        let expected = if closing == T::RBrace {
            "`,` or `}`"
        } else {
            "`,` or `]`"
        };
        let close = self.close(open, closing, expected)?;
        Ok((items, close))
    }

    fn no_comprehension(&mut self, open: Token) -> Parse<()> {
        if self.peek() == T::Reserved && self.text(self.tok()) == "for" {
            let token = self.tok();
            return Err(self.report(
                Diagnostic::error(token.span, "comprehensions are not supported")
                    .with_related(open.span, "the comprehension starts here")
                    .with_help("a query works on all of its rows at once: `where` keeps rows, and a column step changes them"),
            ));
        }
        Ok(())
    }

    fn number(&mut self, token: Token) -> Number {
        let text: String = self.text(token).chars().filter(|&c| c != '_').collect();
        let float = token.kind == T::Float;
        let radix = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
            Some("0x") => 16,
            Some("0o") => 8,
            Some("0b") => 2,
            _ => 10,
        };
        let zero = Number {
            text: "0".to_string(),
            float,
        };
        if float {
            if !text
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                return zero;
            }
            let mut text = text;
            if text.starts_with('.') {
                text.insert(0, '0');
            }
            if let Some(dot) = text.find('.') {
                if !text[dot + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                    text.insert(dot + 1, '0');
                }
            }
            return Number { text, float };
        }
        if radix == 10 {
            if !text.chars().all(|c| c.is_ascii_digit()) {
                return zero;
            }
            let digits = text.trim_start_matches('0');
            return Number {
                text: if digits.is_empty() { "0" } else { digits }.to_string(),
                float,
            };
        }
        let digits = &text[2..];
        match u128::from_str_radix(digits, radix) {
            Ok(value) => Number {
                text: value.to_string(),
                float,
            },
            Err(_) => {
                if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) {
                    self.note(Diagnostic::error(token.span, "this number is too large"));
                }
                zero
            }
        }
    }

    // ---- Strings

    /// Strings side by side, which Python joins into one: `"a" "b"` is `"ab"`.
    fn strings(&mut self) -> Parse<Expr> {
        let first = self.tok();
        let mut span = first.span;
        let mut parts = Vec::new();
        let mut formatted = false;
        while matches!(self.peek(), T::Str | T::FString) {
            let token = self.bump();
            span = span.to(token.span);
            formatted |= token.kind == T::FString;
            self.string_parts(token, &mut parts);
        }
        let kind = if formatted {
            let mut merged: Vec<FPart> = Vec::new();
            for part in parts {
                match (merged.last_mut(), part) {
                    (Some(FPart::Text(text)), FPart::Text(more)) => text.push_str(&more),
                    (_, part) => merged.push(part),
                }
            }
            ExprKind::FString(merged)
        } else {
            ExprKind::Str(
                parts
                    .into_iter()
                    .map(|part| match part {
                        FPart::Text(text) => text,
                        FPart::Value(_) => String::new(),
                    })
                    .collect(),
            )
        };
        Ok(Expr { kind, span })
    }

    /// The text and the values of one string token.
    fn string_parts(&mut self, token: Token, parts: &mut Vec<FPart>) {
        let text = self.text(token);
        let start = token.span.start as usize;
        let prefix_len = text.find(['"', '\'']).unwrap_or(0);
        let prefix = text[..prefix_len].to_ascii_lowercase();
        let raw = prefix.contains('r');
        let quote = text.as_bytes()[prefix_len];
        let triple = text[prefix_len..].starts_with(if quote == b'"' { "\"\"\"" } else { "'''" });
        let body = start + prefix_len + if triple { 3 } else { 1 };
        let end = token.span.end as usize;
        if token.kind == T::Str {
            let (after, closed) = lexer::scan_plain(self.src.as_bytes(), body, end, quote, triple);
            let body_end = if closed {
                after - if triple { 3 } else { 1 }
            } else {
                after
            };
            let value = self.unescape(Span::new(body, body_end.max(body)), raw, false);
            parts.push(FPart::Text(value));
            return;
        }
        let scan = lexer::scan_fstring(self.src, body, end, quote, triple);
        for piece in scan.pieces {
            match piece {
                Piece::Text(span) => {
                    let value = self.unescape(span, raw, true);
                    parts.push(FPart::Text(value));
                }
                Piece::Field { expr, extra } => {
                    if let Some(extra) = extra {
                        self.note(
                            Diagnostic::error(
                                extra,
                                "f-string formats such as `:.2f` and `!r` are not supported yet",
                            )
                            .with_help("to round a number, use `round(x, 2)`"),
                        );
                    }
                    let value = self.field_value(expr);
                    parts.push(FPart::Value(value));
                }
            }
        }
    }

    fn unescape(&mut self, span: Span, raw: bool, fstring: bool) -> String {
        let (value, problems) = lexer::unescape(self.src, span, raw, fstring);
        for (span, message) in problems {
            self.note(Diagnostic::error(span, message));
        }
        value
    }

    /// The expression in an f-string's field, read on its own.
    fn field_value(&mut self, span: Span) -> Expr {
        let lexed = lexer::lex(self.src, span.start as usize, span.end as usize);
        self.diagnostics.extend(lexed.diagnostics);
        let mut inner = Parser::new(self.src, lexed.tokens);
        // As inside brackets: line breaks do not matter.
        inner.brackets = 1;
        let value = if inner.at(T::Eof) {
            inner.note(
                Diagnostic::error(
                    Span::new(span.start as usize - 1, span.end as usize + 1),
                    "this f-string field is empty",
                )
                .with_help("put a value in it, as in `f\"{name}\"`, or write `{{` for a brace"),
            );
            None
        } else {
            match inner.expr() {
                Ok(value) if inner.at(T::Eof) => Some(value),
                Ok(_) => {
                    inner.expected("`}` at the end of the field");
                    None
                }
                Err(Fail) => None,
            }
        };
        self.diagnostics.append(&mut inner.diagnostics);
        value.unwrap_or(Expr {
            kind: ExprKind::Error,
            span,
        })
    }
}

fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr {
        span: left.span.to(right.span),
        kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
    }
}

/// Whether a value is a name or a dotted name.
fn is_path(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Name(_) => true,
        ExprKind::Attribute(base, _) => is_path(base),
        _ => false,
    }
}

/// Whether a value can be what a `from` or a `join` reads.
fn is_source(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Call(function, _) => is_path(function),
        ExprKind::Query(_) => true,
        _ => is_path(expr),
    }
}

fn is_operator(kind: T) -> bool {
    matches!(
        kind,
        T::Plus
            | T::Minus
            | T::Star
            | T::Slash
            | T::DoubleSlash
            | T::Percent
            | T::DoubleStar
            | T::EqEq
            | T::NotEq
            | T::Lt
            | T::LtEq
            | T::Gt
            | T::GtEq
            | T::Assign
            | T::And
            | T::Or
            | T::Not
            | T::In
            | T::Is
            | T::If
            | T::Else
            | T::Comma
    )
}

/// Help for a Python keyword that ButtercupQL does not use.
fn reserved_help(word: &str) -> &'static str {
    match word {
        "lambda" => "a ButtercupQL function is a `def` of steps",
        "for" | "while" => {
            "a query works on all of its rows at once, so there are no loops: `where` keeps rows, and a column step changes them"
        }
        "with" => "to name a query, write `name = (from ...)` before the main query",
        "return" => "a `def` gives its steps as they are, with no `return`",
        "if" | "else" | "elif" => {
            "a value that depends on a condition is written `a if condition else b`"
        }
        _ => "to use it as a name, put it in backticks",
    }
}
