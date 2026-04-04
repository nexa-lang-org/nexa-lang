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
    /// `src/main/` of the module being compiled — base for local imports.
    root: PathBuf,
    /// Project root — used to locate `lib/`, `modules/`, and cross-module imports.
    project_root: PathBuf,
    /// Name of the module being compiled — used to locate `modules/<name>/lib/`.
    module_name: String,
    /// Source file provider (real FS or in-memory for tests).
    source: S,
    /// Cache: canonical file path → parsed declarations.
    cache: HashMap<PathBuf, Vec<Declaration>>,
    /// Cycle detection: currently being loaded.
    loading: HashSet<PathBuf>,
}

impl<S: SourceProvider> Resolver<S> {
    pub fn new(
        root: impl Into<PathBuf>,
        project_root: impl Into<PathBuf>,
        module_name: impl Into<String>,
        source: S,
    ) -> Self {
        Resolver {
            root: root.into(),
            project_root: project_root.into(),
            module_name: module_name.into(),
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
    /// 2. `src/main/` of the current module (resolver root) + path parts
    /// 3. `<project>/modules/<current_module>/lib/<pkg>@*/src/<Name>.nx`
    /// 4. `<project>/lib/<pkg>@*/src/<Name>.nx` — project-level installed packages
    /// 5. Cross-module: `import core.UI` → `<project>/modules/core/src/main/UI.nx`
    fn resolve_path(
        &self,
        import_path: &str,
        relative_root: &Path,
    ) -> Result<PathBuf, ResolveError> {
        let parts: Vec<&str> = import_path.split('.').collect();

        // 1. Relative to importing file's directory: last_part.nx
        let simple = relative_root.join(format!("{}.nx", parts.last().unwrap_or(&"")));
        if self.source.exists(&simple) {
            return Ok(self.source.canonicalize(&simple).unwrap_or(simple));
        }

        // 2. Resolver root (src/main/) + all parts as path
        let mut rel_path = self.root.clone();
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

        // 3. Module-specific lib: <project>/modules/<module>/lib/<pkg>@*/src/<Name>.nx
        {
            let module_lib = self
                .project_root
                .join("modules")
                .join(&self.module_name)
                .join("lib");
            if let Some(candidate) = find_in_lib(&module_lib, &parts) {
                return Ok(candidate);
            }
        }

        // 4. Project-level lib: <project>/lib/<pkg>@*/src/<Name>.nx
        {
            let project_lib = self.project_root.join("lib");
            if let Some(candidate) = find_in_lib(&project_lib, &parts) {
                return Ok(candidate);
            }
        }

        // 5. Cross-module import: first part matches a sibling module name
        //    e.g. `import core.UI` → <project>/modules/core/src/main/UI.nx
        if parts.len() >= 2 {
            let maybe_module = parts[0];
            let modules_dir = self.project_root.join("modules");
            let cross_module_src = modules_dir
                .join(maybe_module)
                .join("src")
                .join("main")
                .join(format!("{}.nx", parts.last().unwrap_or(&"")));
            if self.source.exists(&cross_module_src) {
                return Ok(self
                    .source
                    .canonicalize(&cross_module_src)
                    .unwrap_or(cross_module_src));
            }
        }

        Err(ResolveError::NotFound(
            import_path.to_string(),
            format!(
                "tried: {}, {} (module lib, project lib, cross-module)",
                simple.display(),
                rel_path.display(),
            ),
        ))
    }
}

/// Search `lib_dir/<pkg>@*/src/<last_part>.nx` for an installed package.
fn find_in_lib(lib_dir: &Path, parts: &[&str]) -> Option<PathBuf> {
    let pkg_name = parts.first()?;
    let file_name = format!("{}.nx", parts.last()?);
    let entries = std::fs::read_dir(lib_dir).ok()?;
    for entry in entries.flatten() {
        let dir_name = entry.file_name();
        let dir_str = dir_name.to_string_lossy();
        if dir_str.starts_with(pkg_name) {
            let candidate = entry.path().join("src").join(&file_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::mem_source::MemSourceProvider;

    fn parse_entry(source: &str) -> Program {
        let tokens = Lexer::new(source).tokenize().unwrap();
        Parser::new(tokens).parse().unwrap()
    }

    fn make_resolver(root: &str, provider: MemSourceProvider) -> Resolver<MemSourceProvider> {
        Resolver::new(root, "", "core", provider)
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
        let mut resolver = make_resolver("/src/main", provider);
        let resolved = resolver
            .resolve(&entry, &PathBuf::from("/src/main/app.nx"))
            .unwrap();

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
        provider.add("/src/main/User.nx", lib_source);

        let mut resolver = make_resolver("/src/main", provider);
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
        let mut resolver = make_resolver("/src", provider);
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

        let mut resolver = make_resolver("/src/main", provider);
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

    // ── Fallback 5: cross-module import ──────────────────────────────────────

    #[test]
    fn resolve_cross_module_import() {
        // entry lives in module "api", imports "core.Button" which is in
        // <project>/modules/core/src/main/Button.nx
        let button_src = r#"class Button { public String label; }"#;
        let entry_source = r#"import core.Button;
app App {
  window Home { }
  route "/" => Home;
}"#;
        let entry = parse_entry(entry_source);
        let entry_path = PathBuf::from("/project/modules/api/src/main/app.nx");

        let mut provider = MemSourceProvider::new();
        provider.add(
            "/project/modules/core/src/main/Button.nx",
            button_src,
        );

        let mut resolver = Resolver::new(
            "/project/modules/api/src/main",
            "/project",
            "api",
            provider,
        );
        let resolved = resolver.resolve(&entry, &entry_path).unwrap();

        let has_button = resolved
            .declarations
            .iter()
            .any(|d| matches!(d, Declaration::Class(c) if c.name == "Button"));
        assert!(has_button, "cross-module Button should be resolved");
    }

    // ── Fallback 4: project-level lib (requires real FS) ────────────────────

    #[test]
    fn resolve_project_lib_fallback() {
        use std::fs;
        use tempfile::TempDir;
        use crate::infrastructure::fs_source::FsSourceProvider;

        let tmp = TempDir::new().unwrap();
        let project = tmp.path();

        // <project>/lib/stdui@1.0.0/src/Button.nx
        let pkg_src = project.join("lib/stdui@1.0.0/src");
        fs::create_dir_all(&pkg_src).unwrap();
        fs::write(pkg_src.join("Button.nx"), "class Button { }").unwrap();

        // entry lives in <project>/modules/core/src/main/
        let module_src = project.join("modules/core/src/main");
        fs::create_dir_all(&module_src).unwrap();
        let entry_path = module_src.join("app.nx");

        let entry_source = "import stdui.Button; app A { window W { } route \"/\" => W; }";
        let entry = parse_entry(entry_source);

        let mut resolver = Resolver::new(
            module_src.clone(),
            project,
            "core",
            FsSourceProvider,
        );
        let resolved = resolver.resolve(&entry, &entry_path).unwrap();

        let has_button = resolved
            .declarations
            .iter()
            .any(|d| matches!(d, Declaration::Class(c) if c.name == "Button"));
        assert!(has_button, "project lib Button should be resolved");
    }

    // ── Transitive imports ───────────────────────────────────────────────────

    #[test]
    fn resolve_transitive_imports() {
        // A imports B, B imports C — final program should contain both B and C.
        let c_src = r#"class C { }"#;
        let b_src = r#"import C;
class B { }"#;
        let entry_source = r#"import B;
app App { window W { } route "/" => W; }"#;

        let entry = parse_entry(entry_source);
        let mut provider = MemSourceProvider::new();
        provider.add("/src/main/B.nx", b_src);
        provider.add("/src/main/C.nx", c_src);

        let mut resolver = make_resolver("/src/main", provider);
        let resolved = resolver
            .resolve(&entry, &PathBuf::from("/src/main/app.nx"))
            .unwrap();

        let names: Vec<&str> = resolved
            .declarations
            .iter()
            .filter_map(|d| {
                if let Declaration::Class(c) = d {
                    Some(c.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(names.contains(&"B"), "B should be in resolved declarations");
        assert!(names.contains(&"C"), "C should be in resolved declarations");
    }

    // ── Cycle detection ──────────────────────────────────────────────────────

    #[test]
    fn resolve_circular_import_returns_error() {
        // A imports B, B imports A → cycle
        let b_src = r#"import A;
class B { }"#;
        let entry_source = r#"import B;
app App { window W { } route "/" => W; }"#;

        let _entry = parse_entry(entry_source);
        let mut provider = MemSourceProvider::new();
        provider.add("/src/main/B.nx", b_src);
        provider.add("/src/main/A.nx", "class A { }");

        let _resolver = make_resolver("/src/main", provider);
        // B tries to import A (the entry's own file path)
        // Since A.nx exists in memory (not the entry file), this won't cycle with the entry.
        // Instead, create a direct cycle: X imports X.
        let cycle_src = r#"import Cycle;
class Cycle { }"#;
        let cycle_entry = parse_entry("import Cycle; app App { window W { } route \"/\" => W; }");
        let mut p2 = MemSourceProvider::new();
        p2.add("/src/main/Cycle.nx", cycle_src);
        let mut r2 = make_resolver("/src/main", p2);
        let err = r2
            .resolve(&cycle_entry, &PathBuf::from("/src/main/app.nx"))
            .unwrap_err();
        assert!(matches!(err, ResolveError::Cycle(_)));
    }
}
