use std::collections::HashSet;
use std::path::PathBuf;

use crate::ast::Stmt;
use crate::error::CompileError;
use crate::lexer;
use crate::parser;

#[derive(Debug)]
pub struct ResolvedProgram {
    pub sources: Vec<(String, String)>,
    pub stmts: Vec<Stmt>,
    pub stmt_files: Vec<String>,
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
}

#[derive(Debug)]
pub enum ResolveErrorKind {
    Compile(CompileError),
    Cycle(String),
    Io { path: String, message: String },
}

#[derive(Debug)]
pub struct ResolveError {
    pub kind: ResolveErrorKind,
    pub sources: Vec<(String, String)>,
}

struct Resolver {
    sources: Vec<(String, String)>,
    stmts: Vec<Stmt>,
    stmt_files: Vec<String>,
    imports: Vec<String>,
    std_imports: Vec<String>,
    done: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

fn dedup_push(v: &mut Vec<String>, item: String) {
    if !v.contains(&item) { v.push(item); }
}

pub fn resolve(entry: &str) -> Result<ResolvedProgram, ResolveError> {
    let mut r = Resolver {
        sources: Vec::new(),
        stmts: Vec::new(),
        stmt_files: Vec::new(),
        imports: Vec::new(),
        std_imports: Vec::new(),
        done: HashSet::new(),
        stack: Vec::new(),
    };
    let entry_path = PathBuf::from(entry);
    match r.process(&entry_path, entry) {
        Ok(()) => Ok(ResolvedProgram {
            sources: r.sources,
            stmts: r.stmts,
            stmt_files: r.stmt_files,
            imports: r.imports,
            std_imports: r.std_imports,
        }),
        Err(kind) => Err(ResolveError { kind, sources: r.sources }),
    }
}

impl Resolver {
    /// `display` is the path text used for error messages and
    /// `stmt_files` (as written on the CLI, or as joined from an
    /// importer's directory) -- kept alongside `path` (used for
    /// cycle/dedup identity) so messages read naturally.
    fn process(&mut self, path: &PathBuf, display: &str) -> Result<(), ResolveErrorKind> {
        if self.done.contains(path) {
            return Ok(());
        }
        if self.stack.contains(path) {
            let mut chain: Vec<String> = self.stack.iter().map(|p| p.display().to_string()).collect();
            chain.push(display.to_string());
            return Err(ResolveErrorKind::Cycle(format!("import cycle: {}", chain.join(" -> "))));
        }

        let src = std::fs::read_to_string(path).map_err(|e| ResolveErrorKind::Io {
            path: display.to_string(),
            message: e.to_string(),
        })?;
        self.sources.push((display.to_string(), src.clone()));

        let toks = lexer::lex(&src)
            .map_err(|e| e.with_file(display.to_string()))
            .map_err(ResolveErrorKind::Compile)?;
        let prog = parser::parse(toks)
            .map_err(|e| e.with_file(display.to_string()))
            .map_err(ResolveErrorKind::Compile)?;

        self.stack.push(path.clone());

        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for name in &prog.verb_imports {
            let child_path = dir.join(name);
            let child_display = child_path.display().to_string();
            self.process(&child_path, &child_display)?;
        }

        self.stmt_files.extend(std::iter::repeat(display.to_string()).take(prog.body.len()));
        self.stmts.extend(prog.body);
        for lib in prog.imports { dedup_push(&mut self.imports, lib); }
        for m in prog.std_imports { dedup_push(&mut self.std_imports, m); }

        self.stack.pop();
        self.done.insert(path.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh temp dir per test, named after the test, under
    /// `std::env::temp_dir()` — same convention `tests/e2e.rs` already
    /// uses for filesystem-touching tests.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("verb_resolve_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, name: &str, src: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, src).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn resolves_a_single_file_with_no_imports() {
        let dir = test_dir("single_file");
        let entry = write(&dir, "entry.verb", "assign x 1;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 1);
        assert_eq!(r.stmt_files, vec![entry.clone()]);
        assert!(r.imports.is_empty());
        assert!(r.std_imports.is_empty());
        assert_eq!(r.sources.len(), 1);
    }

    #[test]
    fn splices_a_verb_file_import_before_the_importers_own_statements() {
        let dir = test_dir("splice");
        write(&dir, "lib.verb", "assign y 2;\n");
        let entry = write(&dir, "entry.verb", "import mod lib.verb;\nassign x 1;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 2);
        assert!(r.stmt_files[0].ends_with("lib.verb"), "stmt_files: {:?}", r.stmt_files);
        assert!(r.stmt_files[1].ends_with("entry.verb"), "stmt_files: {:?}", r.stmt_files);
    }

    #[test]
    fn dedups_a_diamond_import() {
        let dir = test_dir("diamond");
        write(&dir, "shared.verb", "assign s 1;\n");
        write(&dir, "a.verb", "import mod shared.verb;\nassign a 1;\n");
        write(&dir, "b.verb", "import mod shared.verb;\nassign b 1;\n");
        let entry = write(
            &dir, "entry.verb",
            "import mod a.verb;\nimport mod b.verb;\nassign e 1;\n",
        );

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 4, "stmt_files: {:?}", r.stmt_files);
        let shared_count = r.stmt_files.iter().filter(|f| f.ends_with("shared.verb")).count();
        assert_eq!(shared_count, 1, "stmt_files: {:?}", r.stmt_files);
    }

    #[test]
    fn detects_an_import_cycle() {
        let dir = test_dir("cycle");
        write(&dir, "a.verb", "import mod b.verb;\n");
        let entry = write(&dir, "b.verb", "import mod a.verb;\n");
        // b.verb imports a.verb, a.verb imports b.verb back -- resolving
        // either one hits the cycle.
        let _ = entry;
        let b_entry = dir.join("b.verb").to_str().unwrap().to_string();

        let err = resolve(&b_entry).unwrap_err();
        assert!(matches!(err.kind, ResolveErrorKind::Cycle(_)), "expected Cycle, file layout ok");
    }

    #[test]
    fn reports_a_missing_import_file() {
        let dir = test_dir("missing");
        let entry = write(&dir, "entry.verb", "import mod missing.verb;\n");

        let err = resolve(&entry).unwrap_err();
        match err.kind {
            ResolveErrorKind::Io { path, .. } => {
                assert!(path.ends_with("missing.verb"), "path: {path}");
            }
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn merges_imports_and_std_imports_from_imported_files() {
        let dir = test_dir("merge_imports");
        write(&dir, "lib.verb", "import mod mathlib;\nimport std io;\n");
        let entry = write(&dir, "entry.verb", "import mod lib.verb;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.imports, vec!["mathlib".to_string()]);
        assert_eq!(r.std_imports, vec!["io".to_string()]);
    }
}
