pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::services::codegen::CodeGenerator;
pub use application::services::packager::{decode_nxb, PackageError};
pub use application::services::parser::Parser;
pub use application::services::resolver::Resolver;
pub use application::services::semantic::SemanticAnalyzer;

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
}

/// Compile a project to a `.nexa` bundle (NXB AST + manifest + signature).
/// Pipeline: Lexer → Parser → Resolver → SemanticAnalyzer → Optimizer → NXB encode.
#[allow(clippy::result_large_err)]
pub fn compile_to_bundle(
    entry: &Path,
    src_root: &Path,
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

    Ok(BundleResult {
        nxb,
        manifest,
        signature,
    })
}

/// Pipeline commun : lex → parse → resolve → semantic → codegen.
/// `entry` est utilisé pour résoudre les imports relatifs au fichier source.
/// `resolver_root` est la racine de recherche pour les imports de packages.
#[allow(clippy::result_large_err)]
fn run_pipeline(
    source: &str,
    entry: &Path,
    resolver_root: &Path,
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
        resolver_root,
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

/// Compile un fichier `.nx` standalone, en résolvant les imports
/// relativement à son répertoire parent.
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
    run_pipeline(&source, path, root)
}

/// Compile un fichier `.nx` dans le contexte d'un projet structuré.
/// `src_root` = `<project>/src/` — racine du Resolver, permet de résoudre
/// `libs/` en plus de `main/`.
#[allow(clippy::result_large_err)]
pub fn compile_project_file(entry: &Path, src_root: &Path) -> Result<CompileResult, CompileError> {
    let source = std::fs::read_to_string(entry).map_err(|e| CompileError {
        span: Span::dummy(),
        kind: CompileErrorKind::Resolve(application::services::resolver::ResolveError::Io(
            entry.display().to_string(),
            e,
        )),
        file: Some(entry.display().to_string()),
        source: None,
    })?;
    run_pipeline(&source, entry, src_root)
}

/// Compile depuis une string (sans résolution d'imports).
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
        assert!(!result.html.is_empty(), "html ne doit pas être vide");
        assert!(!result.js.is_empty(), "js ne doit pas être vide");
    }

    #[test]
    fn compile_str_html_is_valid_document() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.html.contains("<!DOCTYPE html>"),
            "html doit contenir <!DOCTYPE html>"
        );
        assert!(
            result.html.contains(r#"id="app""#),
            "html doit contenir div#app"
        );
        assert!(result.html.contains("app.js"), "html doit charger app.js");
    }

    #[test]
    fn compile_str_js_contains_window_class() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.js.contains("HomePage"),
            "js doit contenir la classe HomePage"
        );
    }

    #[test]
    fn compile_str_js_contains_route() {
        let result = compile_str(MINIMAL_APP).unwrap();
        assert!(
            result.js.contains(r#"_routes["/"]"#),
            "js doit contenir la route /"
        );
    }

    #[test]
    fn compile_str_syntax_error_returns_err() {
        assert!(compile_str("app { SYNTAXE INVALIDE !!! }").is_err());
    }

    #[test]
    fn compile_str_empty_source_returns_err() {
        assert!(compile_str("").is_err());
    }
}
