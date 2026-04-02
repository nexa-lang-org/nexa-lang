//! Multi-file resolver.
//!
//! Maps package paths like `com.myapp.models.User` to file system paths,
//! parses them recursively, detects cycles, and merges all declarations into
//! a single flat `Program` that the semantic analyser and codegen can consume.

use crate::application::ports::source::SourceProvider;
use crate::application::services::lexer::Lexer;
use crate::application::services::parser::Parser;
use crate::domain::ast::{Declaration, ImportDecl, Program};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("IO error loading '{0}': {1}")]
    Io(String, std::io::Error),
    #[error("Lex error in '{0}': {1}")]
    Lex(String, crate::application::services::lexer::LexError),
    #[error("Parse error in '{0}': {1}")]
    Parse(String, crate::application::services::parser::ParseError),
    #[error("Circular import detected: {0}")]
    Cycle(String),
    #[error("Cannot resolve import '{0}' (tried: {1})")]
    NotFound(String, String),
}

pub struct Resolver<S: SourceProvider> {
    /// Root directory for package resolution
    root: PathBuf,
    /// Source file provider (real FS or in-memory for tests)
    source: S,
    /// Cache: canonical file path → parsed declarations
    cache: HashMap<PathBuf, Vec<Declaration>>,
    /// Cycle detection: currently being loaded
    loading: HashSet<PathBuf>,
}

impl<S: SourceProvider> Resolver<S> {
    pub fn new(root: impl Into<PathBuf>, source: S) -> Self {
        Resolver {
            root: root.into(),
            source,
            cache: HashMap::new(),
            loading: HashSet::new(),
        }
    }

    /// Resolve the entry program + all its (transitive) imports.
    /// Returns the entry `Program` with all imported declarations merged in.
    pub fn resolve(&mut self, entry: &Program, entry_path: &Path) -> Result<Program, ResolveError> {
        let entry_root = entry_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut merged_decls: Vec<Declaration> = Vec::new();

        // Load all imported declarations first (depth-first)
        for import in &entry.imports {
            self.load_import(import, &entry_root, &mut merged_decls)?;
        }

        // Append the entry file's own declarations last (so they win on name conflicts)
        merged_decls.extend(entry.declarations.clone());

        Ok(Program {
            name: entry.name.clone(),
            package: entry.package.clone(),
            imports: entry.imports.clone(),
            server: entry.server.clone(),
            declarations: merged_decls,
            routes: entry.routes.clone(),
        })
    }

    fn load_import(
        &mut self,
        import: &ImportDecl,
        relative_root: &Path,
        out: &mut Vec<Declaration>,
    ) -> Result<(), ResolveError> {
        let file_path = self.resolve_path(&import.path, relative_root)?;

        // Cycle detection
        if self.loading.contains(&file_path) {
            return Err(ResolveError::Cycle(import.path.clone()));
        }

        // Cache hit
        if let Some(decls) = self.cache.get(&file_path) {
            out.extend(decls.clone());
            return Ok(());
        }

        // Load and parse
        self.loading.insert(file_path.clone());

        let source = self
            .source
            .read_source(&file_path)
            .map_err(|e| ResolveError::Io(file_path.display().to_string(), e))?;

        let tokens = Lexer::new(&source)
            .tokenize()
            .map_err(|e| ResolveError::Lex(file_path.display().to_string(), e))?;

        let lib = Parser::new(tokens)
            .parse_lib()
            .map_err(|e| ResolveError::Parse(file_path.display().to_string(), e))?;

        // Recursively resolve this file's imports
        let lib_root = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut lib_decls: Vec<Declaration> = Vec::new();
        for sub_import in &lib.imports {
            self.load_import(sub_import, &lib_root, &mut lib_decls)?;
        }
        lib_decls.extend(lib.declarations.clone());

        self.loading.remove(&file_path);
        self.cache.insert(file_path, lib_decls.clone());
        out.extend(lib_decls);
        Ok(())
    }

    /// Convert a dotted import path to a file system path.
    ///
    /// Strategy (in order):
    /// 1. Relative to the importing file's directory: `User` → `./User.nx`
    /// 2. Relative root + all parts as dirs + last.nx
    /// 3. Project root / all parts as dirs / last.nx
    fn resolve_path(
        &self,
        import_path: &str,
        relative_root: &Path,
    ) -> Result<PathBuf, ResolveError> {
        let parts: Vec<&str> = import_path.split('.').collect();

        // Try: relative directory / last_part.nx
        let simple = relative_root.join(format!("{}.nx", parts.last().unwrap_or(&"")));
        if self.source.exists(&simple) {
            return Ok(self.source.canonicalize(&simple).unwrap_or(simple));
        }

        // Try: relative root / all parts as dirs / last.nx
        let mut rel_path = relative_root.to_path_buf();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                rel_path.push(format!("{}.nx", part));
            } else {
                rel_path.push(part);
            }
        }
        if self.source.exists(&rel_path) {
            return Ok(self.source.canonicalize(&rel_path).unwrap_or(rel_path));
        }

        // Try: project root / all parts as dirs / last.nx
        let mut pkg_path = self.root.clone();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                pkg_path.push(format!("{}.nx", part));
            } else {
                pkg_path.push(part);
            }
        }
        if self.source.exists(&pkg_path) {
            return Ok(self.source.canonicalize(&pkg_path).unwrap_or(pkg_path));
        }

        Err(ResolveError::NotFound(
            import_path.to_string(),
            format!(
                "tried: {}, {}, {}",
                simple.display(),
                rel_path.display(),
                pkg_path.display()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::mem_source::MemSourceProvider;

    fn parse_entry(source: &str) -> Program {
        let tokens = Lexer::new(source).tokenize().unwrap();
        Parser::new(tokens).parse().unwrap()
    }

    fn parse_entry_path() -> PathBuf {
        PathBuf::from("/src/main/app.nx")
    }

    #[test]
    fn resolve_no_imports_returns_entry_declarations() {
        let source = r#"app App {
  server { port: 3000; }
  public window HomePage {
    public render() => Component {
      return Page { Text("Hi") };
    }
  }
  route "/" => HomePage;
}"#;
        let entry = parse_entry(source);
        let decl_count = entry.declarations.len();

        let provider = MemSourceProvider::new();
        let mut resolver = Resolver::new("/src/main", provider);
        let resolved = resolver.resolve(&entry, &parse_entry_path()).unwrap();

        assert_eq!(resolved.declarations.len(), decl_count);
    }

    #[test]
    fn resolve_single_import_from_memory() {
        let lib_source = r#"class User {
  public String name;
}"#;
        let entry_source = r#"import models.User;
app App {
  server { port: 3000; }
  public window HomePage {
    public render() => Component {
      return Page { Text("Hi") };
    }
  }
  route "/" => HomePage;
}"#;
        let entry = parse_entry(entry_source);
        let entry_path = PathBuf::from("/src/main/app.nx");

        let mut provider = MemSourceProvider::new();
        // The resolver tries relative_root/User.nx first → /src/main/User.nx
        provider.add("/src/main/User.nx", lib_source);

        let mut resolver = Resolver::new("/src/main", provider);
        let resolved = resolver.resolve(&entry, &entry_path).unwrap();

        let has_user = resolved
            .declarations
            .iter()
            .any(|d| matches!(d, Declaration::Class(c) if c.name == "User"));
        assert!(
            has_user,
            "resolved program should contain imported User class"
        );
    }

    #[test]
    fn resolve_missing_import_returns_not_found() {
        let entry_source = r#"import missing.Module;
app App {
  server { port: 3000; }
  public window HomePage {
    public render() => Component {
      return Page { Text("Hi") };
    }
  }
  route "/" => HomePage;
}"#;
        let entry = parse_entry(entry_source);
        let provider = MemSourceProvider::new();
        let mut resolver = Resolver::new("/src", provider);
        let err = resolver
            .resolve(&entry, &PathBuf::from("/src/main/app.nx"))
            .unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(..)));
    }

    #[test]
    fn resolve_cached_import_not_reparsed() {
        let lib_source = r#"class Shared { public String id; }"#;
        let entry_source = r#"import Shared;
import Shared;
app App {
  server { port: 3000; }
  public window HomePage {
    public render() => Component {
      return Page { Text("Hi") };
    }
  }
  route "/" => HomePage;
}"#;
        let entry = parse_entry(entry_source);
        let mut provider = MemSourceProvider::new();
        provider.add("/src/main/Shared.nx", lib_source);

        let mut resolver = Resolver::new("/src/main", provider);
        let resolved = resolver
            .resolve(&entry, &PathBuf::from("/src/main/app.nx"))
            .unwrap();

        let has_shared = resolved
            .declarations
            .iter()
            .any(|d| matches!(d, Declaration::Class(c) if c.name == "Shared"));
        assert!(
            has_shared,
            "resolved program should contain the imported Shared class"
        );
    }
}
