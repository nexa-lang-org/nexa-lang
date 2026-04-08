//! # nexa-compiler — internal API
//!
//! This crate is the Nexa compiler pipeline used directly by the `nexa` CLI.
//! It is **not** a stable public API: it is consumed exclusively by crates
//! within this workspace (primarily `crates/cli`).
//!
//! **semver exemption**: breaking changes to this crate's public interface do
//! not constitute a semver violation for the workspace. If `nexa-compiler` is
//! ever published to crates.io as a standalone library, a stable API contract
//! must be defined at that point.
//!
//! Downstream consumers outside this workspace should not depend on this crate.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::services::codegen::CodeGenerator;
pub use application::services::packager::{decode_nxb, PackageError};
pub use application::services::parser::Parser;
pub use application::services::resolver::Resolver;
pub use application::services::semantic::SemanticAnalyzer;
pub use application::services::wasm_codegen::{WasmCodegen, WasmCodegenError};

use crate::domain::span::Span;
use std::{fmt, path::Path};

#[derive(Debug)]
pub struct CompileResult {
    pub html: String,
    pub js: String,
}

/// A compiler error with source location and rustc-style display.
#[derive(Debug)]
pub struct CompileError {
    pub kind: CompileErrorKind,
    pub file: Option<String>,
    pub source: Option<String>,
    pub span: Span,
}

/// The underlying error from whichever compilation phase failed.
#[derive(Debug)]
pub enum CompileErrorKind {
    Lex(application::services::lexer::LexError),
    Parse(application::services::parser::ParseError),
    Resolve(application::services::resolver::ResolveError),
    Semantic(application::services::semantic::SemanticError),
    Codegen(application::services::codegen::CodegenError),
}

impl fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lex(e) => write!(f, "{e}"),
            Self::Parse(e) => write!(f, "{e}"),
            Self::Resolve(e) => write!(f, "{e}"),
            Self::Semantic(e) => write!(f, "{e}"),
            Self::Codegen(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "error: {}", self.kind)?;
        if let Some(file) = &self.file {
            if !self.span.is_dummy() {
                writeln!(f, "  --> {}:{}:{}", file, self.span.line, self.span.col)?;
            } else {
                writeln!(f, "  --> {}", file)?;
            }
        }
        if let (Some(src), false) = (&self.source, self.span.is_dummy()) {
            let line_idx = self.span.line.saturating_sub(1) as usize;
            let line_text = src.lines().nth(line_idx).unwrap_or("");
            let pad = format!("{}", self.span.line).len();
            writeln!(f, "{:>pad$} |", "")?;
            writeln!(f, "{} | {}", self.span.line, line_text)?;
            let leading = self.span.col.saturating_sub(1) as usize;
            let underln = "^".repeat(self.span.len.max(1) as usize);
            writeln!(f, "{:>pad$} | {}{}", "", " ".repeat(leading), underln)?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

/// The output of `compile_to_bundle`: a distributable `.nexa` bundle payload.
pub struct BundleResult {
    /// `b"NXB\x01"` magic + bincode-serialized optimized AST.
    pub nxb: Vec<u8>,
    /// Auto-generated `manifest.json` contents.
    pub manifest: String,
    /// Hex-encoded SHA-256 of `nxb || manifest` bytes.
    pub signature: String,
    /// Original `.nx` source of the entry file (used to include readable source in the bundle).
    pub source: String,
    /// Original file name of the entry file (e.g. `"app.nx"`).
    pub source_filename: String,
}

/// Compile a project to a distributable `.nexa` bundle.
///
/// The bundle contains a binary NXB file (optimized AST), a `manifest.json`, and a
/// SHA-256 signature that the CLI verifies before installation.
///
/// Pipeline: Lexer → Parser → Resolver → SemanticAnalyzer → Optimizer → NXB encode.
///
/// - `entry`        — path to the entry `.nx` file
/// - `src_root`     — `modules/<name>/src/main/` of the module being compiled
/// - `project_root` — project root (used for `lib/` and cross-module resolution)
/// - `module_name`  — name of the module being compiled
#[allow(clippy::result_large_err)]
pub fn compile_to_bundle(
    entry: &Path,
    src_root: &Path,
    project_root: &Path,
    module_name: &str,
    app_name: &str,
    app_version: &str,
) -> Result<BundleResult, CompileError> {
    let _span = tracing::debug_span!(
        "compile_to_bundle",
        entry = %entry.display(),
        app = app_name,
        version = app_version,
    )
    .entered();

    use application::services::{optimizer, packager};
    use sha2::{Digest, Sha256};

    let file = entry.display().to_string();
    let source = std::fs::read_to_string(entry).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(application::services::resolver::ResolveError::Io(
            file.clone(),
            e,
        )),
        file: Some(file.clone()),
        source: None,
    })?;
    let src = source.clone();

    tracing::debug!("Lexing");
    let tokens = application::services::lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Lex(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    tracing::debug!(token_count = tokens.len(), "Parsing");
    let program = application::services::parser::Parser::new(tokens)
        .parse()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Parse(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    tracing::debug!("Resolving imports");
    let resolved = application::services::resolver::Resolver::new(
        src_root,
        project_root,
        module_name,
        infrastructure::fs_source::FsSourceProvider,
    )
    .resolve(&program, entry)
    .map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(e),
        file: Some(file.clone()),
        source: None,
    })?;

    tracing::debug!("Semantic analysis");
    let mut analyzer = application::services::semantic::SemanticAnalyzer::new();
    analyzer.analyze(&resolved).map_err(|e| CompileError {
        span: e.span(),
        kind: CompileErrorKind::Semantic(e),
        file: Some(file.clone()),
        source: Some(src.clone()),
    })?;

    tracing::debug!("Optimizing");
    let optimized = optimizer::optimize(resolved);

    let nxb = packager::encode_nxb(&optimized).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Codegen(application::services::codegen::CodegenError::Generic(
            e.to_string(),
        )),
        file: Some(file.clone()),
        source: None,
    })?;

    let nexa_ver = env!("CARGO_PKG_VERSION");
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let manifest = format!(
        r#"{{"name":"{app_name}","version":"{app_version}","nexa_version":"{nexa_ver}","nxb_version":1,"created_at":{created_at}}}"#
    );

    let mut hasher = Sha256::new();
    hasher.update(&nxb);
    hasher.update(manifest.as_bytes());
    let signature = format!("{:x}", hasher.finalize());

    let source_filename = entry
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main.nx".to_string());

    Ok(BundleResult {
        nxb,
        manifest,
        signature,
        source,
        source_filename,
    })
}

/// Common pipeline: lex → parse → resolve → semantic → codegen.
#[allow(clippy::result_large_err)]
fn run_pipeline(
    source: &str,
    entry: &Path,
    src_root: &Path,
    project_root: &Path,
    module_name: &str,
) -> Result<CompileResult, CompileError> {
    let _span = tracing::debug_span!("compile_pipeline", entry = %entry.display()).entered();

    let file = entry.display().to_string();
    let src = source.to_string();

    let tokens = application::services::lexer::Lexer::new(source)
        .tokenize()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Lex(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    let program = application::services::parser::Parser::new(tokens)
        .parse()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Parse(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    let resolved = application::services::resolver::Resolver::new(
        src_root,
        project_root,
        module_name,
        infrastructure::fs_source::FsSourceProvider,
    )
    .resolve(&program, entry)
    .map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(e),
        file: Some(file.clone()),
        source: None,
    })?;

    let mut analyzer = application::services::semantic::SemanticAnalyzer::new();
    analyzer.analyze(&resolved).map_err(|e| CompileError {
        span: e.span(),
        kind: CompileErrorKind::Semantic(e),
        file: Some(file.clone()),
        source: Some(src.clone()),
    })?;

    application::services::codegen::CodeGenerator::new()
        .generate(&resolved)
        .map_err(|e| CompileError {
            span: Span::dummy(),
            kind: CompileErrorKind::Codegen(e),
            file: Some(file.clone()),
            source: None,
        })
}

/// Compile a standalone `.nx` file, resolving imports relative to its parent directory.
#[allow(clippy::result_large_err)]
pub fn compile_file(path: &Path) -> Result<CompileResult, CompileError> {
    let source = std::fs::read_to_string(path).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(application::services::resolver::ResolveError::Io(
            path.display().to_string(),
            e,
        )),
        file: Some(path.display().to_string()),
        source: None,
    })?;
    let root = path.parent().unwrap_or(Path::new("."));
    run_pipeline(&source, path, root, Path::new(""), "")
}

/// Compile a `.nx` file in the context of a structured project (module-aware).
///
/// - `entry`        — path to the entry `.nx` file
/// - `src_root`     — `modules/<name>/src/main/` of the module being compiled
/// - `project_root` — project root (used for `lib/` and cross-module resolution)
/// - `module_name`  — name of the module being compiled
#[allow(clippy::result_large_err)]
pub fn compile_project_file(
    entry: &Path,
    src_root: &Path,
    project_root: &Path,
    module_name: &str,
) -> Result<CompileResult, CompileError> {
    let source = std::fs::read_to_string(entry).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(application::services::resolver::ResolveError::Io(
            entry.display().to_string(),
            e,
        )),
        file: Some(entry.display().to_string()),
        source: None,
    })?;
    run_pipeline(&source, entry, src_root, project_root, module_name)
}

/// The output of the WASM backend: WebAssembly Text format ready for `wat2wasm`.
pub struct WasmCompileResult {
    /// WAT source text.  Assemble with: `wat2wasm app.wat -o app.wasm`
    pub wat: String,
}

/// Compile a `.nx` file in a structured project to WebAssembly Text format.
///
/// Pipeline: Lex → Parse → Resolve → SemanticAnalyzer → Lower (IR) → WASM codegen.
///
/// The returned [`WasmCompileResult::wat`] is a `.wat` source string that can be
/// assembled to binary WASM with `wat2wasm` (from the WABT toolkit) or the `wat`
/// crate.
#[allow(clippy::result_large_err)]
pub fn compile_to_wasm(
    entry: &Path,
    src_root: &Path,
    project_root: &Path,
    module_name: &str,
) -> Result<WasmCompileResult, CompileError> {
    let source = std::fs::read_to_string(entry).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(application::services::resolver::ResolveError::Io(
            entry.display().to_string(),
            e,
        )),
        file: Some(entry.display().to_string()),
        source: None,
    })?;

    let file = entry.display().to_string();
    let src = source.clone();

    let tokens = application::services::lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Lex(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    let program = application::services::parser::Parser::new(tokens)
        .parse()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Parse(e),
            file: Some(file.clone()),
            source: Some(src.clone()),
        })?;

    let resolved = application::services::resolver::Resolver::new(
        src_root,
        project_root,
        module_name,
        infrastructure::fs_source::FsSourceProvider,
    )
    .resolve(&program, entry)
    .map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(e),
        file: Some(file.clone()),
        source: None,
    })?;

    let mut analyzer = application::services::semantic::SemanticAnalyzer::new();
    analyzer.analyze(&resolved).map_err(|e| CompileError {
        span: e.span(),
        kind: CompileErrorKind::Semantic(e),
        file: Some(file.clone()),
        source: Some(src.clone()),
    })?;

    let ir = application::services::lower::lower(&resolved);

    let wat = application::services::wasm_codegen::WasmCodegen::new()
        .generate_wat(&ir)
        .map_err(|e| CompileError {
            span: Span::dummy(),
            kind: CompileErrorKind::Codegen(
                application::services::codegen::CodegenError::Generic(e.to_string()),
            ),
            file: Some(file.clone()),
            source: None,
        })?;

    Ok(WasmCompileResult { wat })
}

/// Compile from a string (no import resolution).
#[allow(clippy::result_large_err)]
pub fn compile_str(source: &str) -> Result<CompileResult, CompileError> {
    let tokens = application::services::lexer::Lexer::new(source)
        .tokenize()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Lex(e),
            file: None,
            source: Some(source.to_string()),
        })?;

    let program = application::services::parser::Parser::new(tokens)
        .parse()
        .map_err(|e| CompileError {
            span: e.span(),
            kind: CompileErrorKind::Parse(e),
            file: None,
            source: Some(source.to_string()),
        })?;

    let mut analyzer = application::services::semantic::SemanticAnalyzer::new();
    analyzer.analyze(&program).map_err(|e| CompileError {
        span: e.span(),
        kind: CompileErrorKind::Semantic(e),
        file: None,
        source: Some(source.to_string()),
    })?;

    application::services::codegen::CodeGenerator::new()
        .generate(&program)
        .map_err(|e| CompileError {
            span: Span::dummy(),
            kind: CompileErrorKind::Codegen(e),
            file: None,
            source: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_APP: &str = r#"app App {
  server { port: 3000; }
  public window HomePage {
    public render() => Component {
      return Page { Text("Hi") };
    }
  }
  route "/" => HomePage;
}"#;

    #[test]
    fn compile_str_produces_html_and_js() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(!result.html.is_empty(), "html must not be empty");
        assert!(!result.js.is_empty(), "js must not be empty");
    }

    #[test]
    fn compile_str_html_is_valid_document() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.html.contains("<!DOCTYPE html>"),
            "html must contain <!DOCTYPE html>"
        );
        assert!(
            result.html.contains(r#"id="app""#),
            "html must contain div#app"
        );
        assert!(result.html.contains("app.js"), "html must load app.js");
    }

    #[test]
    fn compile_str_js_contains_window_class() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.js.contains("HomePage"),
            "js must contain the HomePage class"
        );
    }

    #[test]
    fn compile_str_js_contains_route() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.js.contains(r#"_routes["/"]"#),
            "js must contain the / route"
        );
    }

    #[test]
    fn compile_str_syntax_error_returns_err() {
        assert!(compile_str("app { INVALID SYNTAX !!! }").is_err());
    }

    #[test]
    fn compile_str_empty_source_returns_err() {
        assert!(compile_str("").is_err());
    }

    // ── stdlib parse tests ────────────────────────────────────────────────────

    fn parse_lib_source(src: &str) -> crate::domain::ast::Program {
        use crate::application::services::{lexer::Lexer, parser::Parser};
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        Parser::new(tokens).parse_lib().expect("parse_lib error")
    }

    #[test]
    fn stdlib_io_parses_correctly() {
        let cases: &[(&str, &str)] = &[
            ("Console", include_str!("../../../stdlib/src/io/Console.nx")),
            ("File",    include_str!("../../../stdlib/src/io/File.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "io/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_math_parses_correctly() {
        let cases: &[(&str, &str)] = &[
            ("Math",   include_str!("../../../stdlib/src/math/Math.nx")),
            ("Random", include_str!("../../../stdlib/src/math/Random.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "math/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_str_parses_correctly() {
        let cases: &[(&str, &str)] = &[
            ("Str",           include_str!("../../../stdlib/src/str/Str.nx")),
            ("StringBuilder", include_str!("../../../stdlib/src/str/StringBuilder.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "str/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_net_parses_correctly() {
        let cases: &[(&str, &str)] = &[
            ("HttpClient", include_str!("../../../stdlib/src/net/HttpClient.nx")),
            ("Socket",     include_str!("../../../stdlib/src/net/Socket.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "net/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_server_parses_correctly() {
        let src = include_str!("../../../stdlib/src/server/HttpServer.nx");
        let p = parse_lib_source(src);
        assert!(p.declarations.iter().any(|d| {
            matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == "HttpServer")
        }));
    }

    #[test]
    fn stdlib_process_parses_correctly() {
        let cases: &[(&str, &str)] = &[
            ("Process", include_str!("../../../stdlib/src/process/Process.nx")),
            ("Env",     include_str!("../../../stdlib/src/process/Env.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "process/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_option_parses_correctly() {
        let src = include_str!("../../../stdlib/src/option.nx");
        let p = parse_lib_source(src);
        assert!(p.declarations.iter().any(|d| {
            matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == "Option")
        }));
    }

    #[test]
    fn stdlib_result_parses_correctly() {
        let src = include_str!("../../../stdlib/src/result.nx");
        let p = parse_lib_source(src);
        assert!(p.declarations.iter().any(|d| {
            matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == "Result")
        }));
    }

    #[test]
    fn stdlib_collections_parses_correctly() {
        // 1 class = 1 file — each file must declare exactly the named class.
        let cases: &[(&str, &str)] = &[
            ("Stack",                include_str!("../../../stdlib/src/collections/Stack.nx")),
            ("Queue",                include_str!("../../../stdlib/src/collections/Queue.nx")),
            ("ArrayList",            include_str!("../../../stdlib/src/collections/ArrayList.nx")),
            ("LinkedList",           include_str!("../../../stdlib/src/collections/LinkedList.nx")),
            ("HashMap",              include_str!("../../../stdlib/src/collections/HashMap.nx")),
            ("HashSet",              include_str!("../../../stdlib/src/collections/HashSet.nx")),
            ("Deque",                include_str!("../../../stdlib/src/collections/Deque.nx")),
            ("PriorityQueue",        include_str!("../../../stdlib/src/collections/PriorityQueue.nx")),
            ("SynchronizedStack",    include_str!("../../../stdlib/src/collections/SynchronizedStack.nx")),
            ("SynchronizedQueue",    include_str!("../../../stdlib/src/collections/SynchronizedQueue.nx")),
            ("SynchronizedArrayList",include_str!("../../../stdlib/src/collections/SynchronizedArrayList.nx")),
            ("SynchronizedHashMap",  include_str!("../../../stdlib/src/collections/SynchronizedHashMap.nx")),
            ("SynchronizedHashSet",  include_str!("../../../stdlib/src/collections/SynchronizedHashSet.nx")),
            ("SynchronizedDeque",    include_str!("../../../stdlib/src/collections/SynchronizedDeque.nx")),
        ];
        for (name, src) in cases {
            let p = parse_lib_source(src);
            assert!(
                p.declarations.iter().any(|d| {
                    matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == *name)
                }),
                "collections/{name}.nx must declare class {name}"
            );
        }
    }

    #[test]
    fn stdlib_future_parses_correctly() {
        let src = include_str!("../../../stdlib/src/async/Future.nx");
        let p = parse_lib_source(src);
        assert!(p.declarations.iter().any(|d| {
            matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == "Future")
        }));
    }

    #[test]
    fn stdlib_completable_future_parses_correctly() {
        let src = include_str!("../../../stdlib/src/async/CompletableFuture.nx");
        let p = parse_lib_source(src);
        assert!(p.declarations.iter().any(|d| {
            matches!(d, crate::domain::ast::Declaration::Class(c) if c.name == "CompletableFuture")
        }));
    }

    // ── ADT / enum tests ──────────────────────────────────────────────────────

    fn parse_enum_src(src: &str) -> crate::domain::ast::Program {
        use crate::application::services::{lexer::Lexer, parser::Parser};
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        Parser::new(tokens).parse_lib().expect("parse error")
    }

    #[test]
    fn enum_unit_variants_parse_correctly() {
        let src = "package test; enum Color { Red, Green, Blue }";
        let p = parse_enum_src(src);
        let decl = p.declarations.first().unwrap();
        if let crate::domain::ast::Declaration::Enum(e) = decl {
            assert_eq!(e.name, "Color");
            assert_eq!(e.variants.len(), 3);
            assert_eq!(e.variants[0].name, "Red");
            assert!(e.variants[0].fields.is_empty());
        } else {
            panic!("expected Enum declaration");
        }
    }

    #[test]
    fn enum_tuple_variants_parse_correctly() {
        let src = "package test; enum Shape { Circle(Int), Rectangle(Int, Int), Point }";
        let p = parse_enum_src(src);
        if let crate::domain::ast::Declaration::Enum(e) = &p.declarations[0] {
            assert_eq!(e.variants[0].fields.len(), 1); // Circle(Int)
            assert_eq!(e.variants[1].fields.len(), 2); // Rectangle(Int, Int)
            assert_eq!(e.variants[2].fields.len(), 0); // Point
        } else {
            panic!("expected Enum declaration");
        }
    }

    #[test]
    fn enum_lowers_to_ir_correctly() {
        use crate::application::services::lower;
        let src = "package test; enum Color { Red, Green, Blue }";
        let p = parse_enum_src(src);
        let ir = lower::lower(&p);
        assert_eq!(ir.enums.len(), 1);
        assert_eq!(ir.enums[0].name, "Color");
        assert_eq!(ir.enums[0].variants.len(), 3);
    }

    #[test]
    fn enum_codegen_emits_js_class() {
        use crate::application::services::{codegen::CodeGenerator, lower};
        let src = r#"app App {
  enum Direction { North, South, East, West }
  public window Home {
    public render() => Component { return Page { Text("hi") }; }
  }
  route "/" => Home;
}"#;
        let tokens = crate::application::services::lexer::Lexer::new(src)
            .tokenize()
            .unwrap();
        let program = crate::application::services::parser::Parser::new(tokens)
            .parse()
            .unwrap();
        let _ir = lower::lower(&program);
        let js = CodeGenerator::new().generate(&program).unwrap().js;
        assert!(js.contains("class Direction"), "should emit JS class for enum");
        assert!(js.contains("static North()"), "should emit static factory for North");
        assert!(js.contains("static South()"), "should emit static factory for South");
    }

    // ── Pattern matching tests ────────────────────────────────────────────────

    #[test]
    fn match_stmt_parses_with_wildcard() {
        let src = r#"package test;
public class Foo {
    check(x: Int) => Int {
        match (x) {
            42 => { return 1; }
            _ => { return 0; }
        }
        return 0;
    }
}"#;
        let p = parse_lib_source(src);
        assert_eq!(p.declarations.len(), 1);
    }

    #[test]
    fn match_stmt_parses_enum_pattern() {
        let src = r#"package test;
public class Handler {
    handle(c: Int) => Int {
        match (c) {
            Red => { return 1; }
            Green => { return 2; }
            _ => { return 0; }
        }
        return 0;
    }
}"#;
        let p = parse_lib_source(src);
        assert_eq!(p.declarations.len(), 1);
    }

    #[test]
    fn match_stmt_lowers_to_ir_match() {
        use crate::application::services::lower;
        use crate::domain::ir::IrStmt;
        let src = r#"package test;
public class Foo {
    check(x: Int) => Int {
        match (x) {
            1 => { return 1; }
            _ => { return 0; }
        }
        return 0;
    }
}"#;
        let p = parse_lib_source(src);
        let ir = lower::lower(&p);
        let method = &ir.classes[0].methods[0];
        assert!(
            method.body.iter().any(|s| matches!(s, IrStmt::Match { .. })),
            "match stmt should lower to IrStmt::Match"
        );
    }

    // ── Test block tests ─────────────────────────────────────────────────────

    #[test]
    fn test_blocks_parse_correctly() {
        use crate::application::services::{lexer::Lexer, parser::Parser};
        let src = r#"package mytest;
test "addition works" {
    let x: Int = 1 + 1;
    let y: Int = 2;
}
test "subtraction works" {
    let a: Int = 5 - 3;
}
"#;
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        let p = Parser::new(tokens).parse_lib().expect("parse error");
        assert_eq!(
            p.declarations
                .iter()
                .filter(|d| matches!(d, crate::domain::ast::Declaration::Test(_)))
                .count(),
            2,
            "should find 2 test blocks"
        );
    }

    // ── Error recovery tests ─────────────────────────────────────────────────

    #[test]
    fn parser_collects_zero_errors_for_valid_code() {
        use crate::application::services::{lexer::Lexer, parser::Parser};
        let src = r#"package test;
public class Foo {
    bar() => Int {
        let x: Int = 1 + 1;
        return x;
    }
}"#;
        let tokens = Lexer::new(src).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let p = parser.parse_lib().unwrap();
        assert!(parser.collected_errors().is_empty(), "valid code should produce no collected errors");
        assert_eq!(p.declarations.len(), 1);
    }
}
