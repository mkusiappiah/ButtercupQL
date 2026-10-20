//! The Python package of ButtercupQL: `import buttercupql`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

use buttercupql as core;

pyo3::create_exception!(
    _buttercupql,
    CompileError,
    PyValueError,
    "A ButtercupQL text that does not compile. Its message shows every problem under its line, and `diagnostics` \
     holds them."
);

/// A problem found in a text: what it is, where, and how to put it right.
#[pyclass(frozen, module = "buttercupql")]
struct Diagnostic {
    inner: core::Diagnostic,
    /// The text the problem is in, and its name, for showing it.
    source: Arc<core::SourceText>,
}

impl Diagnostic {
    fn new(inner: core::Diagnostic, main: &Arc<core::SourceText>) -> Diagnostic {
        let source = inner.file.clone().unwrap_or_else(|| main.clone());
        Diagnostic { inner, source }
    }

    fn location(&self) -> (usize, usize) {
        core::SourceFile::new(&self.source.name, &self.source.text).location(self.inner.span.start)
    }
}

#[pymethods]
impl Diagnostic {
    /// `"error"` or `"warning"`.
    #[getter]
    fn severity(&self) -> &'static str {
        self.inner.severity.name()
    }

    /// What is wrong, in one line.
    #[getter]
    fn message(&self) -> &str {
        &self.inner.message
    }

    /// A word shown under the place itself, such as "did you mean `status`?".
    #[getter]
    fn label(&self) -> &str {
        &self.inner.label
    }

    /// How to put it right, when the message and the label do not say.
    #[getter]
    fn help(&self) -> Vec<String> {
        self.inner.help.clone()
    }

    /// The module the problem is in, when it is not the text compiled.
    #[getter]
    fn file(&self) -> Option<String> {
        self.inner.file.as_ref().map(|file| file.name.clone())
    }

    /// The line, counted from 1.
    #[getter]
    fn line(&self) -> usize {
        self.location().0
    }

    /// The column, counted from 1 in characters.
    #[getter]
    fn column(&self) -> usize {
        self.location().1
    }

    /// The problem as `bcql` shows it: its message, and its line with the place marked.
    #[pyo3(signature = (color = false))]
    fn render(&self, color: bool) -> String {
        let file = core::SourceFile::new(&self.source.name, &self.source.text);
        self.inner.render(&file, color)
    }

    fn __str__(&self) -> String {
        self.render(false)
    }

    fn __repr__(&self) -> String {
        let (line, column) = self.location();
        format!(
            "<Diagnostic {} at {}:{line}:{column}: {}>",
            self.inner.severity.name(),
            self.source.name,
            self.inner.message
        )
    }
}

/// An argument of a query `def` compiled with its parameters open.
#[pyclass(frozen, module = "buttercupql")]
struct Arg {
    inner: core::Arg,
}

#[pymethods]
impl Arg {
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// The type its annotation names: `str`, `int`, `float`, `bool`, `date`, `datetime`, or `""` for none.
    #[getter]
    #[pyo3(name = "type")]
    fn ty(&self) -> &'static str {
        match self.inner.ty {
            core::Type::Text => "str",
            core::Type::Int => "int",
            core::Type::Float | core::Type::Decimal => "float",
            core::Type::Bool => "bool",
            core::Type::Date => "date",
            core::Type::Timestamp => "datetime",
            core::Type::Unknown | core::Type::Duration => "",
        }
    }

    /// Its value when none is given, or `None`.
    #[getter]
    fn default(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.inner.default {
            Some(value) => to_python(py, value),
            None => Ok(py.None()),
        }
    }

    fn __repr__(&self) -> String {
        format!("<Arg {}>", self.inner.name)
    }
}

/// A query compiled to SQL.
#[pyclass(frozen, module = "buttercupql")]
struct Compiled {
    inner: core::Compiled,
    source: Arc<core::SourceText>,
}

#[pymethods]
impl Compiled {
    /// The SQL.
    #[getter]
    fn sql(&self) -> &str {
        &self.inner.sql
    }

    /// The values for the placeholders, in order. For a query with arguments, `bind()` gives them.
    #[getter]
    fn params(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let values = self
            .inner
            .values()
            .map_err(|error| PyValueError::new_err(format!("{error}: give it with bind()")))?;
        values_list(py, &values)
    }

    /// Each placeholder's name, in order: `p1`, `p2` for values, and each argument's own name.
    #[getter]
    fn names(&self) -> Vec<String> {
        self.inner.names.clone()
    }

    /// The query's arguments, for a `def` compiled with `query=`.
    #[getter]
    fn args(&self) -> Vec<Arg> {
        self.inner
            .args
            .iter()
            .map(|arg| Arg { inner: arg.clone() })
            .collect()
    }

    /// Problems that do not stop the query compiling, but may mean it does not do what was meant.
    #[getter]
    fn warnings(&self) -> Vec<Diagnostic> {
        self.inner
            .warnings
            .iter()
            .map(|warning| Diagnostic::new(warning.clone(), &self.source))
            .collect()
    }

    /// The values for the placeholders, in order, with the query's arguments given by name. An argument not given
    /// takes its default.
    #[pyo3(signature = (**args))]
    fn bind(&self, py: Python<'_>, args: Option<&Bound<'_, PyDict>>) -> PyResult<Py<PyAny>> {
        let args = arguments(args)?;
        let given: Vec<(&str, core::Value)> = args
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let values = self
            .inner
            .bind(&given)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        values_list(py, &values)
    }

    /// The values for the placeholders by name, each name once: for the `:name` and `{name:Type}` styles.
    #[pyo3(signature = (**args))]
    fn bind_named(&self, py: Python<'_>, args: Option<&Bound<'_, PyDict>>) -> PyResult<Py<PyAny>> {
        let args = arguments(args)?;
        let given: Vec<(&str, core::Value)> = args
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let named = self
            .inner
            .bind_named(&given)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let dict = PyDict::new(py);
        for (name, value) in named {
            dict.set_item(name, to_python(py, &value)?)?;
        }
        Ok(dict.into_any().unbind())
    }

    fn __str__(&self) -> &str {
        &self.inner.sql
    }

    fn __repr__(&self) -> String {
        format!("<Compiled {:?}>", self.inner.sql)
    }
}

/// The tables of a database, to check names against.
#[pyclass(frozen, module = "buttercupql")]
struct Schema {
    inner: core::Schema,
}

#[pymethods]
impl Schema {
    /// Reads the tables from the database's own `CREATE TABLE` statements.
    #[staticmethod]
    fn from_sql(py: Python<'_>, text: &str) -> PyResult<Schema> {
        let source = Arc::new(core::SourceText {
            name: "<schema>".to_string(),
            text: text.to_string(),
        });
        core::Schema::from_sql(text)
            .map(|inner| Schema { inner })
            .map_err(|problems| compile_error(py, problems, &source))
    }

    /// The tables' names.
    #[getter]
    fn tables(&self) -> Vec<String> {
        self.inner
            .tables()
            .iter()
            .map(|table| table.name.clone())
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("<Schema of {}>", self.tables().join(", "))
    }
}

/// Modules kept as texts, by their dotted names.
struct Texts(HashMap<String, String>);

impl core::Loader for Texts {
    fn load(
        &self,
        module: &core::ModuleName<'_>,
        _importer: &str,
    ) -> Result<core::SourceText, String> {
        let name = module.parts.join(".");
        self.0
            .get(&name)
            .map(|text| core::SourceText {
                name: name.clone(),
                text: text.clone(),
            })
            .ok_or_else(|| format!("there is no module `{name}` in `modules`"))
    }
}

/// Where a text's modules come from: the texts given, or the files beside the text's own.
enum Modules {
    Texts(Texts),
    Files(core::FileLoader),
}

impl Modules {
    fn new(modules: Option<&Bound<'_, PyDict>>, path: Option<&str>) -> PyResult<Option<Modules>> {
        if let Some(modules) = modules {
            let mut texts = HashMap::new();
            for (name, text) in modules.iter() {
                texts.insert(name.extract::<String>()?, text.extract::<String>()?);
            }
            return Ok(Some(Modules::Texts(Texts(texts))));
        }
        Ok(path.map(|path| {
            let folder = Path::new(path)
                .parent()
                .filter(|folder| !folder.as_os_str().is_empty())
                .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
            Modules::Files(core::FileLoader::new(folder))
        }))
    }

    fn loader(&self) -> &dyn core::Loader {
        match self {
            Modules::Texts(texts) => texts,
            Modules::Files(files) => files,
        }
    }
}

/// The schema given: `CREATE TABLE` statements, a `Schema`, or a dict of tables, each a dict of its columns' SQL
/// types.
fn schema_of(py: Python<'_>, schema: Option<&Bound<'_, PyAny>>) -> PyResult<Option<core::Schema>> {
    let Some(schema) = schema else {
        return Ok(None);
    };
    if schema.is_none() {
        return Ok(None);
    }
    if let Ok(text) = schema.extract::<String>() {
        return Schema::from_sql(py, &text).map(|schema| Some(schema.inner));
    }
    if let Ok(schema) = schema.extract::<PyRef<'_, Schema>>() {
        return Ok(Some(schema.inner.clone()));
    }
    if schema.is_instance_of::<PyDict>() {
        let mut ddl = String::new();
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        for (table, columns) in schema.extract::<HashMap<String, HashMap<String, String>>>()? {
            let columns: Vec<String> = columns
                .iter()
                .map(|(name, kind)| format!("{} {kind}", quote(name)))
                .collect();
            ddl.push_str(&format!(
                "CREATE TABLE {} ({});\n",
                quote(&table),
                columns.join(", ")
            ));
        }
        return Schema::from_sql(py, &ddl).map(|schema| Some(schema.inner));
    }
    Err(PyTypeError::new_err(
        "schema takes CREATE TABLE statements, a Schema, or a dict of tables",
    ))
}

fn dialect_of(dialect: &str) -> PyResult<core::Dialect> {
    dialect
        .parse()
        .map_err(|error: core::UnknownDialect| PyValueError::new_err(error.to_string()))
}

fn style_of(params: Option<&str>) -> PyResult<Option<core::ParamStyle>> {
    Ok(match params {
        None => None,
        Some("dollar") => Some(core::ParamStyle::Dollar),
        Some("question" | "qmark") => Some(core::ParamStyle::Question),
        Some("format") => Some(core::ParamStyle::Format),
        Some("colon" | "named") => Some(core::ParamStyle::Colon),
        Some("braces") => Some(core::ParamStyle::Braces),
        Some(other) => {
            return Err(PyValueError::new_err(format!(
                "params takes dollar, question, format, colon or braces, not {other:?}"
            )));
        }
    })
}

/// Compiles a ButtercupQL text to SQL for a database.
#[pyfunction]
#[pyo3(signature = (text, dialect = "postgres", schema = None, params = None, query = None, path = None, modules = None))]
#[allow(clippy::too_many_arguments)]
fn compile(
    py: Python<'_>,
    text: &str,
    dialect: &str,
    schema: Option<&Bound<'_, PyAny>>,
    params: Option<&str>,
    query: Option<&str>,
    path: Option<&str>,
    modules: Option<&Bound<'_, PyDict>>,
) -> PyResult<Compiled> {
    let dialect = dialect_of(dialect)?;
    let style = style_of(params)?;
    let schema = schema_of(py, schema)?;
    let loader = Modules::new(modules, path)?;
    let name = path.unwrap_or("<query>");
    let source = Arc::new(core::SourceText {
        name: name.to_string(),
        text: text.to_string(),
    });
    let mut options = core::Options::new(dialect).with_name(name);
    if let Some(schema) = &schema {
        options = options.with_schema(schema);
    }
    if let Some(style) = style {
        options = options.with_params(style);
    }
    if let Some(query) = query {
        options = options.with_query(query);
    }
    if let Some(loader) = &loader {
        options = options.with_loader(loader.loader());
    }
    let compiled =
        core::compile(text, &options).map_err(|problems| compile_error(py, problems, &source))?;
    Ok(Compiled {
        inner: compiled,
        source,
    })
}

/// The problems in a ButtercupQL text, without compiling it to SQL: an empty list means none.
#[pyfunction]
#[pyo3(signature = (text, dialect = "postgres", schema = None, path = None, modules = None))]
fn check(
    py: Python<'_>,
    text: &str,
    dialect: &str,
    schema: Option<&Bound<'_, PyAny>>,
    path: Option<&str>,
    modules: Option<&Bound<'_, PyDict>>,
) -> PyResult<Vec<Diagnostic>> {
    let name = path.unwrap_or("<query>");
    let source = Arc::new(core::SourceText {
        name: name.to_string(),
        text: text.to_string(),
    });
    let parsed = core::parse(text);
    if parsed.has_errors() || parsed.file.query.is_none() {
        return Ok(parsed
            .diagnostics
            .into_iter()
            .map(|problem| Diagnostic::new(problem, &source))
            .collect());
    }
    match compile(py, text, dialect, schema, None, None, path, modules) {
        Ok(compiled) => Ok(compiled.warnings()),
        Err(error) => {
            let diagnostics = error.value(py).getattr("diagnostics")?;
            diagnostics.extract::<Vec<Py<Diagnostic>>>().map(|found| {
                found
                    .into_iter()
                    .map(|problem| {
                        let problem = problem.get();
                        Diagnostic {
                            inner: problem.inner.clone(),
                            source: problem.source.clone(),
                        }
                    })
                    .collect()
            })
        }
    }
}

/// Runs the `bcql` command with its arguments, and returns its exit code.
#[pyfunction]
fn _run(args: Vec<String>) -> u8 {
    bcql::main(&args)
}

/// A `CompileError` holding the problems, its message showing each under its line.
fn compile_error(
    py: Python<'_>,
    problems: Vec<core::Diagnostic>,
    source: &Arc<core::SourceText>,
) -> PyErr {
    let shown: Vec<String> = problems
        .iter()
        .map(|problem| Diagnostic::new(problem.clone(), source).render(false))
        .collect();
    let error = CompileError::new_err(shown.join("\n"));
    let diagnostics: Vec<Diagnostic> = problems
        .into_iter()
        .map(|problem| Diagnostic::new(problem, source))
        .collect();
    let list = diagnostics
        .into_iter()
        .map(|diagnostic| Py::new(py, diagnostic))
        .collect::<PyResult<Vec<_>>>()
        .and_then(|items| PyList::new(py, items));
    if let Ok(list) = list {
        let _ = error.value(py).setattr("diagnostics", list);
    }
    error
}

/// Arguments given to `bind()`, as values.
fn arguments(args: Option<&Bound<'_, PyDict>>) -> PyResult<Vec<(String, core::Value)>> {
    let Some(args) = args else {
        return Ok(Vec::new());
    };
    args.iter()
        .map(|(name, value)| Ok((name.extract::<String>()?, from_python(&value)?)))
        .collect()
}

/// A Python value as a value for a placeholder. A date or a time is passed as text, in the form databases read.
fn from_python(value: &Bound<'_, PyAny>) -> PyResult<core::Value> {
    if value.is_none() {
        return Ok(core::Value::Null);
    }
    if value.is_instance_of::<PyBool>() {
        return Ok(core::Value::Bool(value.extract()?));
    }
    if value.is_instance_of::<PyInt>() {
        return Ok(core::Value::Int(value.extract()?));
    }
    if value.is_instance_of::<PyFloat>() {
        return Ok(core::Value::Float(value.extract()?));
    }
    if value.is_instance_of::<PyString>() {
        return Ok(core::Value::Text(value.extract()?));
    }
    if value.hasattr("isoformat")? {
        let text = if value.hasattr("hour")? && value.hasattr("year")? {
            value.call_method1("isoformat", (" ",))?
        } else {
            value.call_method0("isoformat")?
        };
        return Ok(core::Value::Text(text.extract()?));
    }
    if value.get_type().name()? == "Decimal" {
        return Ok(core::Value::Text(value.str()?.to_string()));
    }
    Err(PyTypeError::new_err(format!(
        "a query's argument is None, a bool, an int, a float, a str, a date or a time, not {}",
        value.get_type().name()?
    )))
}

fn to_python(py: Python<'_>, value: &core::Value) -> PyResult<Py<PyAny>> {
    Ok(match value {
        core::Value::Null => py.None(),
        core::Value::Bool(value) => PyBool::new(py, *value).to_owned().into_any().unbind(),
        core::Value::Int(value) => value.into_pyobject(py)?.into_any().unbind(),
        core::Value::Float(value) => value.into_pyobject(py)?.into_any().unbind(),
        core::Value::Text(text) => text.into_pyobject(py)?.into_any().unbind(),
    })
}

fn values_list(py: Python<'_>, values: &[core::Value]) -> PyResult<Py<PyAny>> {
    let items = values
        .iter()
        .map(|value| to_python(py, value))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(PyList::new(py, items)?.into_any().unbind())
}

#[pymodule]
fn _buttercupql(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", core::VERSION)?;
    module.add("CompileError", module.py().get_type::<CompileError>())?;
    module.add_class::<Diagnostic>()?;
    module.add_class::<Arg>()?;
    module.add_class::<Compiled>()?;
    module.add_class::<Schema>()?;
    module.add_function(wrap_pyfunction!(compile, module)?)?;
    module.add_function(wrap_pyfunction!(check, module)?)?;
    module.add_function(wrap_pyfunction!(_run, module)?)?;
    Ok(())
}
