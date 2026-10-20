//! Modules: the files a ButtercupQL file imports, and how they are found.
//!
//! `import shared` and `from shared import recent` read a module's definitions: its `def`s, named queries and named
//! values. A [`Loader`] finds each module's text, so that a host decides where modules come from: files beside the
//! one compiled, as [`FileLoader`] reads them, or texts it keeps itself.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::ast::{self, ImportKind};
use crate::diagnostic::{Diagnostic, SourceText};
use crate::parser;
use crate::span::Span;

/// A module as an import names it: `shared.dates` is the names `shared` and `dates`, and `.lib` has one dot before
/// its name.
#[derive(Debug, Clone, Copy)]
pub struct ModuleName<'n> {
    /// How many dots come before the names: 0 for `import shared`, 1 for `from .lib import x`, 2 for `from ..lib`.
    pub level: u32,
    /// The names between the dots.
    pub parts: &'n [String],
}

/// Finds the modules a ButtercupQL file imports.
pub trait Loader {
    /// The text of a module, and the name messages show for it. The name also tells modules apart: imports that
    /// come to the same name share one module. `importer` is the name of the text that imports it.
    fn load(&self, module: &ModuleName<'_>, importer: &str) -> Result<SourceText, String>;
}

/// Finds modules as files. `import shared` reads `shared.bcql` in the root folder, which is usually the folder of
/// the file compiled, and `import a.b` reads `a/b.bcql` there. `from .lib import x` reads `lib.bcql` beside the
/// importing file, and each further dot goes one folder up.
#[derive(Debug, Clone)]
pub struct FileLoader {
    root: PathBuf,
}

impl FileLoader {
    /// A loader that finds modules imported without dots in `root`.
    pub fn new(root: impl Into<PathBuf>) -> FileLoader {
        FileLoader { root: root.into() }
    }

    /// The file a module is read from, without reading it. An editor uses it to read the text of a file being
    /// edited rather than the one saved.
    pub fn path(&self, module: &ModuleName<'_>, importer: &str) -> Result<PathBuf, String> {
        let Some((last, folders)) = module.parts.split_last() else {
            return Err("the import names no module".to_string());
        };
        let mut path = if module.level == 0 {
            self.root.clone()
        } else {
            let mut folder = Path::new(importer)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            for _ in 1..module.level {
                folder.push("..");
            }
            folder
        };
        for folder in folders {
            path.push(folder);
        }
        path.push(format!("{last}.bcql"));
        Ok(tidy(&path))
    }
}

impl Loader for FileLoader {
    fn load(&self, module: &ModuleName<'_>, importer: &str) -> Result<SourceText, String> {
        let path = self.path(module, importer)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        Ok(SourceText {
            name: path.display().to_string(),
            text,
        })
    }
}

/// A path with its `.` and `..` folded in, so that two ways of writing one file give one name.
fn tidy(path: &Path) -> PathBuf {
    let mut tidy = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let parent = tidy.file_name().is_some_and(|name| name != "..");
                if parent {
                    tidy.pop();
                } else {
                    tidy.push("..");
                }
            }
            other => tidy.push(other.as_os_str()),
        }
    }
    tidy
}

/// A module, read: the main text is the first.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    pub(crate) source: Arc<SourceText>,
    pub(crate) file: ast::File,
    /// For each import line of the file, in order, the modules it reads: one, or one for each name of
    /// `from . import a, b`. `None` is a module that could not be read, whose problem has been reported.
    pub(crate) imports: Vec<Vec<Option<usize>>>,
}

impl Loaded {
    pub(crate) fn new(source: SourceText, file: ast::File) -> Loaded {
        Loaded {
            source: Arc::new(source),
            file,
            imports: Vec::new(),
        }
    }
}

/// Reads every module the main text imports, and the modules they import, after the main text, which is first.
pub(crate) fn load_all(
    main: Loaded,
    loader: Option<&dyn Loader>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Loaded> {
    let mut reading = Reading {
        loader,
        modules: vec![main],
        diagnostics,
    };
    reading.visit(0, &mut Vec::new());
    reading.modules
}

struct Reading<'l, 'd> {
    loader: Option<&'l dyn Loader>,
    modules: Vec<Loaded>,
    diagnostics: &'d mut Vec<Diagnostic>,
}

impl Reading<'_, '_> {
    fn report(&mut self, module: usize, diagnostic: Diagnostic) {
        let mut diagnostic = diagnostic;
        if module != 0 {
            diagnostic.file = Some(self.modules[module].source.clone());
        }
        self.diagnostics.push(diagnostic);
    }

    /// Reads the imports of a module, and theirs. `stack` holds the modules being read, to find one that imports
    /// itself.
    fn visit(&mut self, index: usize, stack: &mut Vec<usize>) {
        stack.push(index);
        // Each import line, as the modules it names and where it is written.
        let lines: Vec<Vec<(u32, Vec<String>, Span)>> = self.modules[index]
            .file
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Import(import) => Some(match &import.kind {
                    ImportKind::Module { path, .. } => vec![(0, names(path), path.span)],
                    ImportKind::Names {
                        level,
                        module: Some(path),
                        ..
                    } => vec![(*level, names(path), path.span)],
                    ImportKind::Names {
                        level,
                        module: None,
                        names,
                    } => names
                        .iter()
                        .map(|name| (*level, vec![name.name.text.clone()], name.name.span))
                        .collect(),
                }),
                _ => None,
            })
            .collect();
        let importer = self.modules[index].source.name.clone();
        let mut imports = Vec::new();
        for line in lines {
            let mut read = Vec::new();
            for (level, parts, span) in line {
                read.push(self.read(index, &importer, level, &parts, span, stack));
            }
            imports.push(read);
        }
        self.modules[index].imports = imports;
        stack.pop();
    }

    fn read(
        &mut self,
        importer_index: usize,
        importer: &str,
        level: u32,
        parts: &[String],
        span: Span,
        stack: &mut Vec<usize>,
    ) -> Option<usize> {
        let written = format!("{}{}", ".".repeat(level as usize), parts.join("."));
        let Some(loader) = self.loader else {
            let problem = Diagnostic::error(span, format!("`{written}` can't be imported here"))
                .with_label("there is nothing to read modules with")
                .with_help("the `bcql` command reads modules from files; a program gives `Options` a loader");
            self.report(importer_index, problem);
            return None;
        };
        let name = ModuleName { level, parts };
        let source = match loader.load(&name, importer) {
            Ok(source) => source,
            Err(message) => {
                let problem = Diagnostic::error(span, format!("can't import `{written}`"))
                    .with_label(message);
                self.report(importer_index, problem);
                return None;
            }
        };
        if let Some(found) = self
            .modules
            .iter()
            .position(|module| module.source.name == source.name)
        {
            if stack.contains(&found) {
                let problem = Diagnostic::error(span, format!("`{written}` imports itself"))
                    .with_label("this import leads back to a module that imports it")
                    .with_help(
                        "move what both modules use into a third module that imports neither",
                    );
                self.report(importer_index, problem);
                return None;
            }
            return Some(found);
        }
        let (file, problems) = parser::parse(&source.text);
        let index = self.modules.len();
        self.modules.push(Loaded::new(source, file));
        for problem in problems {
            self.report(index, problem);
        }
        self.visit(index, stack);
        Some(index)
    }
}

fn names(path: &ast::Path) -> Vec<String> {
    path.parts.iter().map(|part| part.text.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_tidied_so_one_file_has_one_name() {
        assert_eq!(
            tidy(Path::new("a/./b/../c.bcql")),
            PathBuf::from("a/c.bcql")
        );
        assert_eq!(tidy(Path::new("../x.bcql")), PathBuf::from("../x.bcql"));
        assert_eq!(tidy(Path::new("a/b/../../c.bcql")), PathBuf::from("c.bcql"));
    }
}
