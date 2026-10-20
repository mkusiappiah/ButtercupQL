//! `bcql lsp`: a language server, so that an editor shows each problem as it is typed, completes names, describes
//! them, and goes to where one is defined. It speaks the Language Server Protocol over standard input and output.
//!
//! An editor's settings say which database the queries are for and where its schema is:
//! `{"dialect": "sqlite", "schema": "schema.sql"}`, given when the server starts or when the settings change. A
//! schema's path is read from the workspace's folder.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use buttercupql::ast::{self, ExprKind, ImportKind, Item, StepKind};
use buttercupql::builtins::{self, Builtin};
use buttercupql::{
    Column, Diagnostic, Dialect, FileLoader, Loader, ModuleName, Options, Param, Schema, Severity,
    SourceFile, SourceText, Span, Table, Value,
};

use crate::json::{Json, object};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const SERVER_NOT_INITIALIZED: i64 = -32002;

/// The largest message read, so that a wrong length can't take all the memory.
const LARGEST_MESSAGE: usize = 64 << 20;

/// Runs the server until the editor stops it, and returns the exit code.
pub(crate) fn run() -> u8 {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut server = Server::default();
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) => return if server.shut_down { 0 } else { 1 },
            Err(error) => {
                eprintln!("bcql lsp: {error}");
                return 1;
            }
        };
        let replies = match Json::parse(&message) {
            Ok(message) => server.handle(&message),
            Err(problem) => vec![error_reply(&Json::Null, PARSE_ERROR, &problem)],
        };
        for reply in replies {
            if let Err(error) = write_message(&mut output, &reply.to_string()) {
                eprintln!("bcql lsp: can't write to the editor: {error}");
                return 1;
            }
        }
        if let Some(code) = server.exit {
            return code;
        }
    }
}

/// Reads one message: its headers, a blank line, and as many bytes as its `Content-Length` says. `None` is the end of
/// the input.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let value: usize = value
                    .trim()
                    .parse()
                    .map_err(|_| invalid("a Content-Length that is not a number"))?;
                if value > LARGEST_MESSAGE {
                    return Err(invalid("a message larger than 64 MiB"));
                }
                length = Some(value);
            }
        }
    }
    let mut body = vec![0; length.unwrap_or_default()];
    input.read_exact(&mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|_| invalid("a message that is not UTF-8"))
}

fn write_message(output: &mut impl Write, body: &str) -> io::Result<()> {
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

fn reply(id: &Json, result: Json) -> Json {
    object([
        ("jsonrpc", "2.0".into()),
        ("id", id.clone()),
        ("result", result),
    ])
}

fn error_reply(id: &Json, code: i64, message: &str) -> Json {
    object([
        ("jsonrpc", "2.0".into()),
        ("id", id.clone()),
        (
            "error",
            object([("code", code.into()), ("message", message.into())]),
        ),
    ])
}

fn notification(method: &str, params: Json) -> Json {
    object([
        ("jsonrpc", "2.0".into()),
        ("method", method.into()),
        ("params", params),
    ])
}

/// A file being edited: its text as the editor has it, which may not be saved yet.
struct Document {
    text: String,
    /// Its path, for a file on disk.
    path: Option<PathBuf>,
}

/// The schema, read from its file again when the file changes.
#[derive(Default)]
struct SchemaFile {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    schema: Option<Schema>,
    /// Why it could not be read, as last told to the editor.
    problem: Option<String>,
}

#[derive(Default)]
struct Server {
    documents: BTreeMap<String, Document>,
    /// The problems last sent for each file, so that they are sent again only when they change.
    published: BTreeMap<String, String>,
    dialect: Option<Dialect>,
    schema_setting: Option<String>,
    schema: SchemaFile,
    /// The workspace's folder.
    root: Option<PathBuf>,
    /// Whether places in a line are counted in UTF-8 bytes, rather than in UTF-16 units as the protocol's default.
    utf8: bool,
    initialized: bool,
    shut_down: bool,
    exit: Option<u8>,
    /// Messages for the editor to show, sent with the next replies.
    messages: Vec<Json>,
}

impl Server {
    /// Handles a message from the editor, and gives the messages to send back.
    fn handle(&mut self, message: &Json) -> Vec<Json> {
        let Some(method) = message.get("method").as_str() else {
            // A reply to a request of the server's: it sends none that wait for one.
            return Vec::new();
        };
        let id = message.get("id");
        let request = !id.is_null();
        let params = message.get("params");
        if method == "exit" {
            self.exit = Some(if self.shut_down { 0 } else { 1 });
            return Vec::new();
        }
        if !self.initialized && method != "initialize" {
            return if request {
                vec![error_reply(
                    id,
                    SERVER_NOT_INITIALIZED,
                    "the server has not been initialized",
                )]
            } else {
                Vec::new()
            };
        }
        if self.shut_down {
            return if request {
                vec![error_reply(id, INVALID_REQUEST, "the server has shut down")]
            } else {
                Vec::new()
            };
        }
        let mut out = match method {
            "initialize" => {
                let result = self.initialize(params);
                vec![reply(id, result)]
            }
            "initialized" => Vec::new(),
            "shutdown" => {
                self.shut_down = true;
                vec![reply(id, Json::Null)]
            }
            "textDocument/didOpen" => {
                let document = params.get("textDocument");
                if let Some(uri) = document.get("uri").as_str() {
                    let text = document.get("text").as_str().unwrap_or_default();
                    self.documents.insert(
                        uri.to_string(),
                        Document {
                            text: text.to_string(),
                            path: path_of(uri),
                        },
                    );
                }
                self.check_all()
            }
            "textDocument/didChange" => {
                let uri = params.get("textDocument").get("uri").as_str();
                let utf8 = self.utf8;
                if let Some(document) = uri.and_then(|uri| self.documents.get_mut(uri)) {
                    for change in params.get("contentChanges").items() {
                        let new = change.get("text").as_str().unwrap_or_default();
                        let range = change.get("range");
                        if range.is_null() {
                            document.text = new.to_string();
                        } else {
                            let lines = Lines::new(&document.text, utf8);
                            let start = lines.offset(range.get("start"));
                            let end = lines.offset(range.get("end"));
                            document
                                .text
                                .replace_range(start.min(end)..start.max(end), new);
                        }
                    }
                }
                self.check_all()
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => self.check_all(),
            "textDocument/didClose" => {
                let mut out = Vec::new();
                if let Some(uri) = params.get("textDocument").get("uri").as_str() {
                    self.documents.remove(uri);
                    if self.published.remove(uri).is_some() {
                        out.push(publish(uri, Vec::new()));
                    }
                }
                out.extend(self.check_all());
                out
            }
            "workspace/didChangeConfiguration" => {
                self.configure(params.get("settings"));
                self.check_all()
            }
            "textDocument/completion" => {
                let result = self.completion(params);
                vec![reply(id, result)]
            }
            "textDocument/hover" => {
                let result = self.hover(params);
                vec![reply(id, result)]
            }
            "textDocument/definition" => {
                let result = self.definition(params);
                vec![reply(id, result)]
            }
            "textDocument/documentSymbol" => {
                let result = self.symbols(params);
                vec![reply(id, result)]
            }
            "buttercupql/compile" => {
                let result = self.compile(params);
                vec![reply(id, result)]
            }
            _ if request => vec![error_reply(
                id,
                METHOD_NOT_FOUND,
                &format!("bcql has no method `{method}`"),
            )],
            // Notifications the server has no use for, such as `$/cancelRequest`.
            _ => Vec::new(),
        };
        let mut messages = std::mem::take(&mut self.messages);
        messages.append(&mut out);
        messages
    }

    fn initialize(&mut self, params: &Json) -> Json {
        self.initialized = true;
        self.utf8 = params
            .get("capabilities")
            .get("general")
            .get("positionEncodings")
            .items()
            .iter()
            .any(|encoding| encoding.as_str() == Some("utf-8"));
        let root = params
            .get("workspaceFolders")
            .items()
            .first()
            .and_then(|folder| folder.get("uri").as_str())
            .or_else(|| params.get("rootUri").as_str());
        self.root = root
            .and_then(path_of)
            .or_else(|| params.get("rootPath").as_str().map(PathBuf::from));
        self.configure(params.get("initializationOptions"));
        object([
            (
                "capabilities",
                object([
                    (
                        "positionEncoding",
                        Json::from(if self.utf8 { "utf-8" } else { "utf-16" }),
                    ),
                    (
                        "textDocumentSync",
                        object([
                            ("openClose", true.into()),
                            // Changes come as the parts of the text that changed.
                            ("change", 2u32.into()),
                            ("save", object([("includeText", false.into())])),
                        ]),
                    ),
                    (
                        "completionProvider",
                        object([("triggerCharacters", vec![".".into()].into())]),
                    ),
                    ("hoverProvider", true.into()),
                    ("definitionProvider", true.into()),
                    ("documentSymbolProvider", true.into()),
                ]),
            ),
            (
                "serverInfo",
                object([
                    ("name", "bcql".into()),
                    ("version", buttercupql::VERSION.into()),
                ]),
            ),
        ])
    }

    /// Takes the editor's settings: `{"dialect": ..., "schema": ...}`, or the same under `"buttercupql"`. A setting
    /// left out takes its default.
    fn configure(&mut self, settings: &Json) {
        let nested = settings.get("buttercupql");
        let settings = if nested.is_null() { settings } else { nested };
        self.dialect = None;
        if let Some(name) = settings.get("dialect").as_str() {
            match name.parse::<Dialect>() {
                Ok(dialect) => self.dialect = Some(dialect),
                Err(problem) => self.show(1, &format!("ButtercupQL: {problem}")),
            }
        }
        self.schema_setting = settings
            .get("schema")
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .map(str::to_string);
    }

    /// Asks the editor to show a message: 1 is an error, 2 a warning.
    fn show(&mut self, kind: u32, message: &str) {
        self.messages.push(notification(
            "window/showMessage",
            object([("type", kind.into()), ("message", message.into())]),
        ));
    }

    /// Reads the schema again if its setting or its file has changed.
    fn load_schema(&mut self) {
        let path = self.schema_setting.as_ref().map(|setting| {
            let path = PathBuf::from(setting);
            match &self.root {
                Some(root) if path.is_relative() => root.join(path),
                _ => path,
            }
        });
        let modified = path.as_ref().and_then(|path| {
            std::fs::metadata(path)
                .and_then(|data| data.modified())
                .ok()
        });
        if path == self.schema.path
            && modified == self.schema.modified
            && self.schema.path.is_some()
        {
            return;
        }
        let mut schema = SchemaFile {
            path: path.clone(),
            modified,
            ..SchemaFile::default()
        };
        if let Some(path) = &path {
            match std::fs::read_to_string(path) {
                Ok(text) => match Schema::from_sql(&text) {
                    Ok(read) => schema.schema = Some(read),
                    Err(problems) => {
                        let name = path.display().to_string();
                        let first = problems
                            .first()
                            .map(|problem| problem.render(&SourceFile::new(&name, &text), false))
                            .unwrap_or_default();
                        schema.problem = Some(format!(
                            "ButtercupQL: the schema in {name} can't be read, so names are not checked against it.\n{first}"
                        ));
                    }
                },
                Err(error) => {
                    schema.problem = Some(format!(
                        "ButtercupQL: can't read the schema {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        if schema.problem.is_some() && schema.problem != self.schema.problem {
            let problem = schema.problem.clone().unwrap_or_default();
            self.show(2, &problem);
        }
        self.schema = schema;
    }

    /// Checks every open file, and sends the problems of each whose problems have changed. A file can change what
    /// another finds when one imports the other, so all are checked.
    fn check_all(&mut self) -> Vec<Json> {
        self.load_schema();
        let mut out = Vec::new();
        let uris: Vec<String> = self.documents.keys().cloned().collect();
        for uri in uris {
            let diagnostics = self.diagnostics(&uri);
            let text = Json::Array(diagnostics.clone()).to_string();
            if self.published.get(&uri) != Some(&text) {
                self.published.insert(uri.clone(), text);
                out.push(publish(&uri, diagnostics));
            }
        }
        out
    }

    /// The name a file goes by in messages, which is also where its imports are found from.
    fn name_of(&self, uri: &str) -> String {
        match self
            .documents
            .get(uri)
            .and_then(|document| document.path.as_ref())
        {
            Some(path) => path.display().to_string(),
            None => uri.to_string(),
        }
    }

    /// Where the modules a file imports are read from: the file's folder, or for a file not saved yet, the
    /// workspace's.
    fn files_for(&self, uri: &str) -> FileLoader {
        match self
            .documents
            .get(uri)
            .and_then(|document| document.path.as_ref())
        {
            Some(path) => crate::loader_for(&path.display().to_string()),
            None => FileLoader::new(self.root.clone().unwrap_or_else(|| PathBuf::from("."))),
        }
    }

    fn diagnostics(&self, uri: &str) -> Vec<Json> {
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let name = self.name_of(uri);
        let loader = Open {
            files: self.files_for(uri),
            documents: &self.documents,
        };
        let mut options = Options::new(self.dialect.unwrap_or(Dialect::Postgres))
            .with_name(&name)
            .with_loader(&loader);
        if let Some(schema) = &self.schema.schema {
            options = options.with_schema(schema);
        }
        let problems = crate::problems(&document.text, &options);
        let lines = Lines::new(&document.text, self.utf8);
        let parsed = buttercupql::parse(&document.text);
        problems
            .iter()
            .map(|problem| self.diagnostic(problem, &lines, uri, &name, &parsed.file, &loader))
            .collect()
    }

    /// A problem as the protocol gives it. A problem in an imported module is shown on the import that reads it, and
    /// says where it is in the module.
    fn diagnostic(
        &self,
        problem: &Diagnostic,
        lines: &Lines<'_>,
        uri: &str,
        name: &str,
        file: &ast::File,
        loader: &Open<'_>,
    ) -> Json {
        let mut message = problem.message.clone();
        if !problem.label.is_empty() && problem.label != problem.message {
            message.push('\n');
            message.push_str(&problem.label);
        }
        for help in &problem.help {
            message.push_str("\nhelp: ");
            message.push_str(help);
        }
        let mut related = Vec::new();
        let range = match &problem.file {
            None => {
                for label in &problem.related {
                    related.push(object([
                        (
                            "location",
                            object([("uri", uri.into()), ("range", lines.range(label.span))]),
                        ),
                        ("message", label.message.as_str().into()),
                    ]));
                }
                lines.range(problem.span)
            }
            Some(module) => {
                let module_lines = Lines::new(&module.text, self.utf8);
                let module_uri = uri_of(Path::new(&module.name));
                related.push(object([
                    (
                        "location",
                        object([
                            ("uri", module_uri.as_str().into()),
                            ("range", module_lines.range(problem.span)),
                        ]),
                    ),
                    ("message", problem.message.as_str().into()),
                ]));
                for label in &problem.related {
                    related.push(object([
                        (
                            "location",
                            object([
                                ("uri", module_uri.as_str().into()),
                                ("range", module_lines.range(label.span)),
                            ]),
                        ),
                        ("message", label.message.as_str().into()),
                    ]));
                }
                message = format!("in {}: {message}", module.name);
                let import = imports(file)
                    .into_iter()
                    .find(|(module_name, _)| {
                        loader
                            .files
                            .path(&module_name.borrowed(), name)
                            .is_ok_and(|path| path.display().to_string() == module.name)
                    })
                    .or_else(|| imports(file).into_iter().next())
                    .map_or(Span::default(), |(_, span)| span);
                lines.range(import)
            }
        };
        let mut fields = vec![
            ("range".to_string(), range),
            (
                "severity".to_string(),
                Json::from(match problem.severity {
                    Severity::Error => 1u32,
                    Severity::Warning => 2u32,
                }),
            ),
            ("source".to_string(), "bcql".into()),
            ("message".to_string(), message.into()),
        ];
        if !related.is_empty() {
            fields.push(("relatedInformation".to_string(), related.into()));
        }
        Json::Object(fields)
    }

    /// The text of an open file and where the request points in it.
    fn place<'s>(&'s self, params: &Json) -> Option<(&'s str, &'s str, usize)> {
        let uri = params.get("textDocument").get("uri").as_str()?;
        let (uri, document) = self.documents.get_key_value(uri)?;
        let lines = Lines::new(&document.text, self.utf8);
        let offset = lines.offset(params.get("position"));
        Some((uri.as_str(), document.text.as_str(), offset))
    }

    fn completion(&self, params: &Json) -> Json {
        let Some((uri, text, offset)) = self.place(params) else {
            return Json::Null;
        };
        let schema = self.schema.schema.as_ref();
        let parsed = buttercupql::parse(text);
        let start = word_start(text, offset);
        let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
        let before = &text[line_start..start];
        let mut items = Vec::new();
        if let Some(path) = before.strip_suffix('.') {
            let path = dotted_before(path);
            let tables = tables_before(text, line_start);
            match self.attribute_of(&path, &tables, schema, uri, &parsed.file) {
                Some(Attributes::Columns(table)) => {
                    for column in &table.columns {
                        items.push(column_item(column));
                    }
                    for column in &table.columns {
                        if let Some(link) = column.link() {
                            items.push(link_item(link, column));
                        }
                    }
                }
                Some(Attributes::Module(names)) => {
                    for (name, kind, detail) in names {
                        items.push(item(&name, kind, &detail, ""));
                    }
                }
                None => {
                    for method in builtins::METHODS {
                        items.push(builtin_item(method, METHOD));
                    }
                    for part in builtins::DATE_PARTS {
                        items.push(builtin_item(part, PROPERTY));
                    }
                }
            }
            return Json::Array(items);
        }
        let trimmed = before.trim();
        if trimmed.is_empty() {
            for step in builtins::STEPS {
                items.push(builtin_item(step, KEYWORD));
            }
            for word in ["def", "import"] {
                items.push(item(word, KEYWORD, "", ""));
            }
            for def in defs(&parsed.file) {
                if !is_query_def(def) {
                    items.push(def_item(def, text));
                }
            }
            return Json::Array(items);
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if matches!(words.as_slice(), ["from"] | ["join"] | ["left", "join"]) {
            if let Some(schema) = schema {
                for table in schema.tables() {
                    items.push(table_item(table));
                }
            }
            for named in &parsed.file.items {
                match named {
                    Item::Let(named) if matches!(named.value.kind, ExprKind::Query(_)) => {
                        items.push(item(&named.name.text, VARIABLE, "a named query", ""));
                    }
                    Item::Def(def) if is_query_def(def) => items.push(def_item(def, text)),
                    _ => {}
                }
            }
            return Json::Array(items);
        }
        // In an expression: the columns of the tables read, the functions, and the names the file defines.
        let tables = tables_before(text, line_start);
        if let Some(schema) = schema {
            for name in &tables {
                if let Some(table) = schema.table(name) {
                    for column in &table.columns {
                        items.push(column_item(column));
                        if let Some(link) = column.link() {
                            items.push(link_item(link, column));
                        }
                    }
                }
            }
        }
        for function in builtins::FUNCTIONS {
            items.push(builtin_item(function, FUNCTION));
        }
        for named in &parsed.file.items {
            if let Item::Let(named) = named {
                if !matches!(named.value.kind, ExprKind::Query(_)) {
                    items.push(item(&named.name.text, CONSTANT, "a value", ""));
                }
            }
            if let Item::Def(def) = named {
                if def.span.range().contains(&offset) {
                    for param in &def.params {
                        items.push(item(&param.name.text, VARIABLE, "a parameter", ""));
                    }
                }
            }
        }
        for word in builtins::KEYWORDS {
            items.push(item(word, KEYWORD, "", ""));
        }
        Json::Array(items)
    }

    /// What a dotted name before a `.` holds: a table's columns, or a module's names.
    fn attribute_of<'s>(
        &self,
        path: &[&str],
        tables: &[String],
        schema: Option<&'s Schema>,
        uri: &str,
        file: &ast::File,
    ) -> Option<Attributes<'s>> {
        let (first, rest) = path.split_first()?;
        if rest.is_empty() {
            if let Some(names) = self.module_names(first, uri, file) {
                return Some(Attributes::Module(names));
            }
        }
        let schema = schema?;
        // A table read by name, or a link from one of the tables read.
        let mut table = schema.table(first).or_else(|| {
            tables.iter().find_map(|name| {
                let column = schema
                    .table(name)?
                    .columns
                    .iter()
                    .find(|column| column.link() == Some(*first))?;
                schema.table(&column.references.as_ref()?.table)
            })
        })?;
        for part in rest {
            let column = table
                .columns
                .iter()
                .find(|column| column.link() == Some(*part))?;
            table = schema.table(&column.references.as_ref()?.table)?;
        }
        Some(Attributes::Columns(table))
    }

    /// The names of a module the file imports as a whole, `import shared`, with what each is.
    fn module_names(
        &self,
        name: &str,
        uri: &str,
        file: &ast::File,
    ) -> Option<Vec<(String, u32, String)>> {
        let path = file.items.iter().find_map(|item| match item {
            Item::Import(ast::Import {
                kind: ImportKind::Module { path, alias },
                ..
            }) => {
                let written = alias.as_ref().map_or(path.parts.last()?, |alias| alias);
                (written.text == name).then_some(path)
            }
            _ => None,
        })?;
        let parts: Vec<String> = path.parts.iter().map(|part| part.text.clone()).collect();
        let source = self.read_module(0, &parts, uri)?;
        let parsed = buttercupql::parse(&source.text);
        Some(
            parsed
                .file
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Def(def) => Some((
                        def.name.text.clone(),
                        FUNCTION,
                        first_line(&source.text, def.span).to_string(),
                    )),
                    Item::Let(named) => Some((
                        named.name.text.clone(),
                        if matches!(named.value.kind, ExprKind::Query(_)) {
                            VARIABLE
                        } else {
                            CONSTANT
                        },
                        first_line(&source.text, named.span).to_string(),
                    )),
                    Item::Import(_) => None,
                })
                .collect(),
        )
    }

    /// A module a file imports, read as an import would read it.
    fn read_module(&self, level: u32, parts: &[String], uri: &str) -> Option<SourceText> {
        let loader = Open {
            files: self.files_for(uri),
            documents: &self.documents,
        };
        loader
            .load(&ModuleName { level, parts }, &self.name_of(uri))
            .ok()
    }

    fn hover(&self, params: &Json) -> Json {
        let Some((uri, text, offset)) = self.place(params) else {
            return Json::Null;
        };
        let start = word_start(text, offset);
        let end = word_end(text, offset);
        if start == end {
            return Json::Null;
        }
        let word = &text[start..end];
        let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
        let before = &text[line_start..start];
        let after = text[end..].trim_start_matches([' ', '\t']);
        let schema = self.schema.schema.as_ref();
        let parsed = buttercupql::parse(text);
        let tables = tables_before(text, line_start);
        let shown = if let Some(path) = before.strip_suffix('.') {
            let path = dotted_before(path);
            match self.attribute_of(&path, &tables, schema, uri, &parsed.file) {
                Some(Attributes::Columns(table)) => table
                    .column(word)
                    .map(|column| describe_column(&table.name, column))
                    .or_else(|| {
                        let column = table
                            .columns
                            .iter()
                            .find(|column| column.link() == Some(word))?;
                        Some(describe_link(&table.name, word, column))
                    }),
                Some(Attributes::Module(names)) => names
                    .into_iter()
                    .find(|(name, _, _)| name == word)
                    .map(|(_, _, detail)| code(&detail)),
                None => builtins::find(builtins::METHODS, word)
                    .or_else(|| builtins::find(builtins::DATE_PARTS, word))
                    .map(describe),
            }
        } else if before.trim().is_empty() && builtins::find(builtins::STEPS, word).is_some() {
            builtins::find(builtins::STEPS, word).map(describe)
        } else if before.trim() == "left" && word == "join" {
            builtins::find(builtins::STEPS, "left").map(describe)
        } else if let Some(shown) = self.describe_name(word, text, &parsed.file, offset) {
            Some(shown)
        } else if after.starts_with('(') {
            builtins::find(builtins::FUNCTIONS, word).map(describe)
        } else if let Some(table) = schema.and_then(|schema| schema.table(word)) {
            Some(describe_table(table))
        } else {
            let column = schema.and_then(|schema| {
                tables.iter().find_map(|name| {
                    let table = schema.table(name)?;
                    let column = table
                        .column(word)
                        .map(|column| describe_column(&table.name, column));
                    column.or_else(|| {
                        let link = table
                            .columns
                            .iter()
                            .find(|column| column.link() == Some(word))?;
                        Some(describe_link(&table.name, word, link))
                    })
                })
            });
            column.or_else(|| builtins::find(builtins::FUNCTIONS, word).map(describe))
        };
        match shown {
            Some(value) => {
                let lines = Lines::new(text, self.utf8);
                object([
                    (
                        "contents",
                        object([("kind", "markdown".into()), ("value", value.into())]),
                    ),
                    ("range", lines.range(Span::new(start, end))),
                ])
            }
            None => Json::Null,
        }
    }

    /// A name the file defines, or a parameter of the `def` the place is in, described.
    fn describe_name(
        &self,
        word: &str,
        text: &str,
        file: &ast::File,
        offset: usize,
    ) -> Option<String> {
        for item in &file.items {
            match item {
                Item::Def(def) if def.name.text == word => {
                    let mut shown = code(first_line(text, def.span).trim_end_matches(':'));
                    if let Some(docstring) = &def.docstring {
                        shown.push_str("\n\n");
                        shown.push_str(docstring.value.trim());
                    }
                    return Some(shown);
                }
                Item::Def(def) if def.span.range().contains(&offset) => {
                    if let Some(param) = def.params.iter().find(|param| param.name.text == word) {
                        return Some(format!(
                            "{}\n\na parameter of `{}`",
                            code(&text[param.span.range()]),
                            def.name.text
                        ));
                    }
                }
                Item::Let(named) if named.name.text == word => {
                    let shown = if matches!(named.value.kind, ExprKind::Query(_)) {
                        format!("{}\n\na named query", code(&format!("{word} = (...)")))
                    } else {
                        code(first_line(text, named.span))
                    };
                    return Some(shown);
                }
                _ => {}
            }
        }
        None
    }

    fn definition(&self, params: &Json) -> Json {
        let Some((uri, text, offset)) = self.place(params) else {
            return Json::Null;
        };
        let start = word_start(text, offset);
        let end = word_end(text, offset);
        if start == end {
            return Json::Null;
        }
        let word = &text[start..end];
        let parsed = buttercupql::parse(text);
        let lines = Lines::new(text, self.utf8);
        let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
        let before = &text[line_start..start];
        if let Some(path) = before.strip_suffix('.') {
            // `shared.recent`: a name of a module imported whole.
            let path = dotted_before(path);
            if let [module] = path.as_slice() {
                let import = parsed.file.items.iter().find_map(|item| match item {
                    Item::Import(ast::Import {
                        kind: ImportKind::Module { path, alias },
                        ..
                    }) => {
                        let written = alias.as_ref().map_or(path.parts.last()?, |alias| alias);
                        (written.text == *module).then_some(path)
                    }
                    _ => None,
                });
                if let Some(path) = import {
                    let parts: Vec<String> =
                        path.parts.iter().map(|part| part.text.clone()).collect();
                    return self.definition_in(0, &parts, word, uri);
                }
            }
            return Json::Null;
        }
        for item in &parsed.file.items {
            match item {
                Item::Def(def) if def.name.text == word => {
                    return location(uri, lines.range(def.name.span));
                }
                Item::Let(named) if named.name.text == word => {
                    return location(uri, lines.range(named.name.span));
                }
                Item::Import(import) => {
                    // A name imported from a module, or the module named in the import.
                    match &import.kind {
                        ImportKind::Names {
                            level,
                            module,
                            names,
                        } => {
                            for name in names {
                                let written = name.alias.as_ref().unwrap_or(&name.name);
                                if written.text != word && !name.name.span.range().contains(&offset)
                                {
                                    continue;
                                }
                                return match module {
                                    Some(path) => {
                                        let parts: Vec<String> = path
                                            .parts
                                            .iter()
                                            .map(|part| part.text.clone())
                                            .collect();
                                        self.definition_in(*level, &parts, &name.name.text, uri)
                                    }
                                    // `from . import shared` names a module.
                                    None => self.module_location(
                                        *level,
                                        std::slice::from_ref(&name.name.text),
                                        uri,
                                    ),
                                };
                            }
                            if let Some(path) = module {
                                if path.span.range().contains(&offset) {
                                    let parts: Vec<String> =
                                        path.parts.iter().map(|part| part.text.clone()).collect();
                                    return self.module_location(*level, &parts, uri);
                                }
                            }
                        }
                        ImportKind::Module { path, .. } if path.span.range().contains(&offset) => {
                            let parts: Vec<String> =
                                path.parts.iter().map(|part| part.text.clone()).collect();
                            return self.module_location(0, &parts, uri);
                        }
                        ImportKind::Module { .. } => {}
                    }
                }
                _ => {}
            }
        }
        Json::Null
    }

    /// Where a module defines a name.
    fn definition_in(&self, level: u32, parts: &[String], name: &str, uri: &str) -> Json {
        let Some(source) = self.read_module(level, parts, uri) else {
            return Json::Null;
        };
        let parsed = buttercupql::parse(&source.text);
        let lines = Lines::new(&source.text, self.utf8);
        let module_uri = uri_of(Path::new(&source.name));
        for item in &parsed.file.items {
            match item {
                Item::Def(def) if def.name.text == name => {
                    return location(&module_uri, lines.range(def.name.span));
                }
                Item::Let(named) if named.name.text == name => {
                    return location(&module_uri, lines.range(named.name.span));
                }
                _ => {}
            }
        }
        Json::Null
    }

    /// The start of a module's file.
    fn module_location(&self, level: u32, parts: &[String], uri: &str) -> Json {
        match self.read_module(level, parts, uri) {
            Some(source) => {
                let lines = Lines::new(&source.text, self.utf8);
                location(
                    &uri_of(Path::new(&source.name)),
                    lines.range(Span::empty(0)),
                )
            }
            None => Json::Null,
        }
    }

    fn symbols(&self, params: &Json) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str() else {
            return Json::Null;
        };
        let Some(document) = self.documents.get(uri) else {
            return Json::Null;
        };
        let text = &document.text;
        let parsed = buttercupql::parse(text);
        let lines = Lines::new(text, self.utf8);
        let mut symbols = Vec::new();
        for item in &parsed.file.items {
            let (name, kind, span, detail) = match item {
                Item::Def(def) => (
                    &def.name,
                    FUNCTION_SYMBOL,
                    def.span,
                    if is_query_def(def) {
                        "a query"
                    } else {
                        "steps"
                    },
                ),
                Item::Let(named) if matches!(named.value.kind, ExprKind::Query(_)) => {
                    (&named.name, VARIABLE_SYMBOL, named.span, "a named query")
                }
                Item::Let(named) => (&named.name, CONSTANT_SYMBOL, named.span, "a value"),
                Item::Import(_) => continue,
            };
            symbols.push(object([
                ("name", name.text.as_str().into()),
                ("detail", detail.into()),
                ("kind", kind.into()),
                ("range", lines.range(span)),
                ("selectionRange", lines.range(name.span)),
            ]));
        }
        if let Some(query) = &parsed.file.query {
            symbols.push(object([
                ("name", "query".into()),
                ("detail", "the file's query".into()),
                ("kind", EVENT_SYMBOL.into()),
                ("range", lines.range(query.span)),
                (
                    "selectionRange",
                    lines.range(Span::empty(query.span.start as usize)),
                ),
            ]));
        }
        Json::Array(symbols)
    }

    /// The SQL a file compiles to, for an editor to show beside it: `{"sql", "params", "args", "warnings"}`, or
    /// `{"problems"}` with each problem as `bcql` shows it.
    fn compile(&self, params: &Json) -> Json {
        let Some(uri) = params.get("textDocument").get("uri").as_str() else {
            return Json::Null;
        };
        let Some(document) = self.documents.get(uri) else {
            return Json::Null;
        };
        let name = self.name_of(uri);
        let loader = Open {
            files: self.files_for(uri),
            documents: &self.documents,
        };
        let dialect = self.dialect.unwrap_or(Dialect::Postgres);
        let mut options = Options::new(dialect).with_name(&name).with_loader(&loader);
        if let Some(schema) = &self.schema.schema {
            options = options.with_schema(schema);
        }
        let query = params.get("query").as_str();
        if let Some(query) = query {
            options = options.with_query(query);
        }
        let source = SourceFile::new(&name, &document.text);
        let render = |problems: &[Diagnostic]| -> Vec<Json> {
            problems
                .iter()
                .map(|problem| {
                    let shown = match &problem.file {
                        Some(module) => {
                            problem.render(&SourceFile::new(&module.name, &module.text), false)
                        }
                        None => problem.render(&source, false),
                    };
                    shown.into()
                })
                .collect()
        };
        match buttercupql::compile(&document.text, &options) {
            Ok(compiled) => object([
                ("dialect", dialect.name().into()),
                ("sql", compiled.sql.as_str().into()),
                (
                    "params",
                    compiled
                        .params
                        .iter()
                        .map(|param| match param {
                            Param::Value(value) => value_json(value),
                            Param::Arg(name) => object([("arg", name.as_str().into())]),
                        })
                        .collect::<Vec<_>>()
                        .into(),
                ),
                (
                    "args",
                    compiled
                        .args
                        .iter()
                        .map(|arg| {
                            object([
                                ("name", arg.name.as_str().into()),
                                ("type", arg.ty.describe().into()),
                                ("default", arg.default.as_ref().map(value_json).into()),
                            ])
                        })
                        .collect::<Vec<_>>()
                        .into(),
                ),
                ("warnings", render(&compiled.warnings).into()),
            ]),
            Err(problems) => object([
                ("dialect", dialect.name().into()),
                ("problems", render(&problems).into()),
            ]),
        }
    }
}

/// Reads modules as files, taking the text of a file open in the editor over the one saved.
struct Open<'d> {
    files: FileLoader,
    documents: &'d BTreeMap<String, Document>,
}

impl Loader for Open<'_> {
    fn load(&self, module: &ModuleName<'_>, importer: &str) -> Result<SourceText, String> {
        let path = self.files.path(module, importer)?;
        let open = self
            .documents
            .values()
            .find(|document| document.path.as_deref() == Some(path.as_path()));
        match open {
            Some(document) => Ok(SourceText {
                name: path.display().to_string(),
                text: document.text.clone(),
            }),
            None => self.files.load(module, importer),
        }
    }
}

/// A module's name as an import writes it, owning its parts.
struct ImportedModule {
    level: u32,
    parts: Vec<String>,
}

impl ImportedModule {
    fn borrowed(&self) -> ModuleName<'_> {
        ModuleName {
            level: self.level,
            parts: &self.parts,
        }
    }
}

/// The modules a file imports, each with where its import is written.
fn imports(file: &ast::File) -> Vec<(ImportedModule, Span)> {
    let parts = |path: &ast::Path| path.parts.iter().map(|part| part.text.clone()).collect();
    let mut found = Vec::new();
    for item in &file.items {
        let Item::Import(import) = item else {
            continue;
        };
        match &import.kind {
            ImportKind::Module { path, .. } => found.push((
                ImportedModule {
                    level: 0,
                    parts: parts(path),
                },
                import.span,
            )),
            ImportKind::Names {
                level,
                module: Some(path),
                ..
            } => found.push((
                ImportedModule {
                    level: *level,
                    parts: parts(path),
                },
                import.span,
            )),
            ImportKind::Names {
                level,
                module: None,
                names,
            } => {
                for name in names {
                    found.push((
                        ImportedModule {
                            level: *level,
                            parts: vec![name.name.text.clone()],
                        },
                        import.span,
                    ));
                }
            }
        }
    }
    found
}

/// What comes after a dot.
enum Attributes<'s> {
    Columns(&'s Table),
    Module(Vec<(String, u32, String)>),
}

fn publish(uri: &str, diagnostics: Vec<Json>) -> Json {
    notification(
        "textDocument/publishDiagnostics",
        object([("uri", uri.into()), ("diagnostics", diagnostics.into())]),
    )
}

fn location(uri: &str, range: Json) -> Json {
    object([("uri", uri.into()), ("range", range)])
}

fn defs(file: &ast::File) -> impl Iterator<Item = &ast::Def> {
    file.items.iter().filter_map(|item| match item {
        Item::Def(def) => Some(def),
        _ => None,
    })
}

/// A `def` whose body starts with `from` is a query; any other holds steps.
fn is_query_def(def: &ast::Def) -> bool {
    matches!(
        def.body.first().map(|step| &step.kind),
        Some(StepKind::From(_))
    )
}

/// The tables a query reads, by the `from` and `join` lines before a place: the nearest `from` and the joins after
/// it.
fn tables_before(text: &str, line_start: usize) -> Vec<String> {
    let mut tables = Vec::new();
    for line in text[..line_start].lines().rev() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let table = match words.as_slice() {
            ["from", table, rest @ ..] if !rest.contains(&"import") => {
                tables.push(name_part(table));
                break;
            }
            ["join", table, ..] | ["left", "join", table, ..] => name_part(table),
            _ => continue,
        };
        tables.push(table);
    }
    tables.reverse();
    tables
}

/// The name at the start of a word: `cases` of `cases;`.
fn name_part(word: &str) -> String {
    word.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// The dotted name that ends a text: `account.bank` of `where account.bank`.
fn dotted_before(text: &str) -> Vec<&str> {
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '.'))
        .map_or(0, |(at, c)| at + c.len_utf8());
    text[start..]
        .split('.')
        .filter(|part| !part.is_empty())
        .collect()
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_start(text: &str, offset: usize) -> usize {
    text[..offset]
        .char_indices()
        .rev()
        .find(|(_, c)| !is_name_char(*c))
        .map_or(0, |(at, c)| at + c.len_utf8())
}

fn word_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .char_indices()
        .find(|(_, c)| !is_name_char(*c))
        .map_or(text.len(), |(at, _)| offset + at)
}

fn first_line(text: &str, span: Span) -> &str {
    let text = &text[span.range()];
    text.lines().next().unwrap_or_default().trim_end()
}

fn code(text: &str) -> String {
    format!("```python\n{text}\n```")
}

fn describe(builtin: &Builtin) -> String {
    format!("{}\n\n{}", code(builtin.signature), builtin.summary)
}

fn describe_table(table: &Table) -> String {
    let columns: Vec<String> = table
        .columns
        .iter()
        .map(|column| format!("`{}`", column.name))
        .collect();
    format!(
        "{}\n\nColumns: {}",
        code(&format!("table {}", table.name)),
        columns.join(", ")
    )
}

fn column_detail(column: &Column) -> String {
    format!(
        "{}{}",
        column.ty.describe(),
        if column.nullable {
            ", can be missing"
        } else {
            ", never missing"
        }
    )
}

fn describe_column(table: &str, column: &Column) -> String {
    let mut shown = format!(
        "{}\n\n{}",
        code(&format!("{table}.{}", column.name)),
        column_detail(column)
    );
    if let (Some(reference), Some(link)) = (&column.references, column.link()) {
        shown.push_str(&format!(
            "\n\nPoints to `{}.{}`: follow it as `{link}`.",
            reference.table, reference.column
        ));
    }
    shown
}

fn describe_link(table: &str, link: &str, column: &Column) -> String {
    let target = column
        .references
        .as_ref()
        .map_or("", |reference| reference.table.as_str());
    format!(
        "{}\n\nA link: the row of `{target}` that `{}` points to. Read its columns with a dot: `{link}.name`.",
        code(&format!("{table}.{link}")),
        column.name
    )
}

// The kinds of completion items and of symbols, as the protocol numbers them.
const METHOD: u32 = 2;
const FUNCTION: u32 = 3;
const FIELD: u32 = 5;
const VARIABLE: u32 = 6;
const CLASS: u32 = 7;
const PROPERTY: u32 = 10;
const KEYWORD: u32 = 14;
const REFERENCE: u32 = 18;
const CONSTANT: u32 = 21;
const FUNCTION_SYMBOL: u32 = 12;
const VARIABLE_SYMBOL: u32 = 13;
const CONSTANT_SYMBOL: u32 = 14;
const EVENT_SYMBOL: u32 = 24;

fn item(label: &str, kind: u32, detail: &str, documentation: &str) -> Json {
    let mut fields = vec![
        ("label".to_string(), label.into()),
        ("kind".to_string(), kind.into()),
    ];
    if !detail.is_empty() {
        fields.push(("detail".to_string(), detail.into()));
    }
    if !documentation.is_empty() {
        fields.push((
            "documentation".to_string(),
            object([("kind", "markdown".into()), ("value", documentation.into())]),
        ));
    }
    Json::Object(fields)
}

fn builtin_item(builtin: &Builtin, kind: u32) -> Json {
    item(builtin.name, kind, builtin.signature, builtin.summary)
}

fn def_item(def: &ast::Def, text: &str) -> Json {
    let documentation = def
        .docstring
        .as_ref()
        .map_or("", |docstring| docstring.value.trim());
    item(
        &def.name.text,
        FUNCTION,
        first_line(text, def.span).trim_end_matches(':'),
        documentation,
    )
}

fn table_item(table: &Table) -> Json {
    let columns: Vec<&str> = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    item(
        &table.name,
        CLASS,
        &format!("table: {}", columns.join(", ")),
        "",
    )
}

fn column_item(column: &Column) -> Json {
    item(&column.name, FIELD, &column_detail(column), "")
}

/// A value for a placeholder, as JSON.
fn value_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(value) => Json::Bool(*value),
        Value::Int(value) => Json::from(*value),
        Value::Float(value) => Json::from(*value),
        Value::Text(text) => Json::from(text.as_str()),
    }
}

fn link_item(link: &str, column: &Column) -> Json {
    let target = column
        .references
        .as_ref()
        .map_or("", |reference| reference.table.as_str());
    item(
        link,
        REFERENCE,
        &format!("a link to {target}, by {}", column.name),
        "",
    )
}

/// A file's path from its `file:` URI.
fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest)?;
    let bytes = decoded.as_bytes();
    // `/c:/Users` is a Windows drive's path.
    let path = if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        decoded[1..].to_string()
    } else if decoded.starts_with('/') {
        decoded
    } else {
        // `file://server/share` is a path on another machine.
        format!("//{decoded}")
    };
    Some(PathBuf::from(path))
}

/// A path's `file:` URI.
fn uri_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    let mut uri = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Where each line of a text starts, to turn byte offsets into the protocol's lines and places and back.
struct Lines<'t> {
    text: &'t str,
    starts: Vec<usize>,
    utf8: bool,
}

impl<'t> Lines<'t> {
    fn new(text: &'t str, utf8: bool) -> Lines<'t> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Lines { text, starts, utf8 }
    }

    fn position(&self, offset: usize) -> Json {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let start = self.starts[line];
        let character = if self.utf8 {
            offset - start
        } else {
            self.text[start..offset].encode_utf16().count()
        };
        object([("line", line.into()), ("character", character.into())])
    }

    fn range(&self, span: Span) -> Json {
        object([
            ("start", self.position(span.start as usize)),
            ("end", self.position(span.end as usize)),
        ])
    }

    /// The byte offset of a position. A place past the end of its line is the line's end.
    fn offset(&self, position: &Json) -> usize {
        let line = position.get("line").as_u64().unwrap_or_default() as usize;
        let character = position.get("character").as_u64().unwrap_or_default() as usize;
        let Some(&start) = self.starts.get(line) else {
            return self.text.len();
        };
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        let line_text = self.text[start..end].trim_end_matches('\r');
        if self.utf8 {
            let mut at = character.min(line_text.len());
            while !line_text.is_char_boundary(at) {
                at -= 1;
            }
            return start + at;
        }
        let mut units = 0;
        for (at, c) in line_text.char_indices() {
            if units >= character {
                return start + at;
            }
            units += c.len_utf16();
        }
        start + line_text.len()
    }
}

#[cfg(test)]
mod tests;
