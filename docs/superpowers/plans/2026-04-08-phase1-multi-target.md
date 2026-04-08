# Phase 1 — Extension `module.json` + cible Backend/CLI Rust

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `ModuleConfig` with `type`/`platforms`, wire a build dispatcher, and produce a working native Rust binary for modules of type `backend` or `cli`.

**Architecture:** `AppType`/`Platform` types in `project.rs` drive a new `targets/` module in the CLI crate. The compiler crate gains a `compile_to_ir` function and a `codegen_rust.rs` that transpiles `IrModule` → Rust source + `Cargo.toml`. The build command calls `build_module()` which dispatches to the right target per platform.

**Tech Stack:** Rust, Serde (JSON deserialization), Rayon (parallel platform builds), Cargo (invoked as subprocess for compilation)

---

## File Map

**Files à créer :**
- `crates/cli/src/application/targets/mod.rs`
- `crates/cli/src/application/targets/dispatcher.rs`
- `crates/cli/src/application/targets/web.rs`
- `crates/cli/src/application/targets/rust.rs`
- `crates/cli/src/application/targets/desktop.rs`
- `crates/compiler/src/application/services/codegen_rust.rs`

**Files à modifier :**
- `crates/cli/src/application/project.rs` — add AppType, Platform, DesktopConfig, extend ModuleConfig, add effective_platforms()
- `crates/cli/src/application/mod.rs` — declare `targets` module
- `crates/cli/src/application/commands/build.rs` — call build_module() instead of compile_project_file directly
- `crates/cli/src/application/commands/module.rs` — add --type and --platforms flags
- `crates/cli/src/application/commands/init.rs` — add --type flag
- `crates/cli/src/interfaces/cli.rs` — wire new flags into Clap
- `crates/cli/src/application/commands/mod.rs` — re-export module_add with new signature
- `crates/compiler/src/application/services/mod.rs` — declare codegen_rust
- `crates/compiler/src/lib.rs` — add compile_to_ir public function
- `crates/cli/Cargo.toml` — add rayon

---

## Task 1 — Extend `project.rs` : AppType, Platform, DesktopConfig

**Files:**
- Modify: `crates/cli/src/application/project.rs`

- [ ] **Step 1.1 : Écrire les tests unitaires**

Ajouter dans le bloc `#[cfg(test)]` existant à la fin de `project.rs` :

```rust
    #[test]
    fn parse_module_config_type_backend() {
        let json = r#"{"name":"api","main":"app.nx","type":"backend"}"#;
        let cfg = parse_module_config(json, "api").unwrap();
        assert_eq!(cfg.app_type, AppType::Backend);
    }

    #[test]
    fn parse_module_config_type_defaults_to_web() {
        let json = r#"{"name":"core","main":"app.nx"}"#;
        let cfg = parse_module_config(json, "core").unwrap();
        assert_eq!(cfg.app_type, AppType::Web);
    }

    #[test]
    fn parse_module_config_platforms() {
        let json = r#"{"name":"api","main":"app.nx","type":"backend","platforms":["native-linux","native-macos"]}"#;
        let cfg = parse_module_config(json, "api").unwrap();
        assert_eq!(cfg.platforms, vec![Platform::NativeLinux, Platform::NativeMacos]);
    }

    #[test]
    fn effective_platforms_backend_default() {
        let json = r#"{"name":"api","main":"app.nx","type":"backend"}"#;
        let cfg = parse_module_config(json, "api").unwrap();
        assert_eq!(cfg.effective_platforms(), vec![Platform::Native]);
    }

    #[test]
    fn effective_platforms_web_default() {
        let json = r#"{"name":"core","main":"app.nx"}"#;
        let cfg = parse_module_config(json, "core").unwrap();
        assert_eq!(cfg.effective_platforms(), vec![Platform::Browser]);
    }

    #[test]
    fn effective_platforms_explicit_overrides_default() {
        let json = r#"{"name":"api","main":"app.nx","type":"backend","platforms":["native-linux"]}"#;
        let cfg = parse_module_config(json, "api").unwrap();
        assert_eq!(cfg.effective_platforms(), vec![Platform::NativeLinux]);
    }

    #[test]
    fn effective_platforms_package_empty() {
        let json = r#"{"name":"lib","main":"lib.nx","type":"package","version":"1.0.0"}"#;
        let cfg = parse_module_config(json, "lib").unwrap();
        assert!(cfg.effective_platforms().is_empty());
    }

    #[test]
    fn platform_as_str_roundtrip() {
        assert_eq!(Platform::Browser.as_str(), "browser");
        assert_eq!(Platform::NativeLinux.as_str(), "native-linux");
        assert_eq!(Platform::Macos.as_str(), "macos");
    }

    #[test]
    fn parse_module_config_desktop_config() {
        let json = r#"{
            "name":"app","main":"app.nx","type":"desktop",
            "desktop":{"title":"MyApp","width":1200,"height":800}
        }"#;
        let cfg = parse_module_config(json, "app").unwrap();
        let desktop = cfg.desktop.unwrap();
        assert_eq!(desktop.title, "MyApp");
        assert_eq!(desktop.width, 1200);
        assert!(desktop.resizable); // default true
    }
```

- [ ] **Step 1.2 : Lancer les tests — vérifier qu'ils échouent**

```bash
cargo test -p nexa --lib -- project::tests 2>&1 | tail -20
```

Expected: FAIL — `AppType`, `Platform`, `DesktopConfig` not defined yet.

- [ ] **Step 1.3 : Ajouter les types dans `project.rs`**

Après la ligne `use serde::Deserialize;` en haut du fichier, ajouter :

```rust
// ── App types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AppType {
    #[default]
    Web,
    Backend,
    Cli,
    Desktop,
    Package,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    Browser,
    Native,
    NativeMacos,
    NativeWindows,
    NativeLinux,
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Browser       => "browser",
            Platform::Native        => "native",
            Platform::NativeMacos   => "native-macos",
            Platform::NativeWindows => "native-windows",
            Platform::NativeLinux   => "native-linux",
            Platform::Macos         => "macos",
            Platform::Windows       => "windows",
            Platform::Linux         => "linux",
            Platform::Ios           => "ios",
            Platform::Android       => "android",
        }
    }
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DesktopConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_true")]
    pub resizable: bool,
    pub icon: Option<String>,
}
```

- [ ] **Step 1.4 : Étendre `ModuleConfig` et ajouter `effective_platforms()`**

Remplacer la struct `ModuleConfig` existante et son `impl` par :

```rust
/// Deserialized from `modules/<name>/module.json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModuleConfig {
    pub name: String,
    /// Entry file name inside `src/main/`, e.g. `"app.nx"`.
    pub main: String,
    /// Module-specific dependencies (installed in `modules/<name>/lib/`).
    #[serde(default)]
    pub dependencies: HashMap<String, String>,

    // ── Multi-target fields ───────────────────────────────────────────────────
    /// Application type. Determines which codegen target is used.
    /// Absent in module.json → defaults to `Web` (full rétro-compat).
    #[serde(default, rename = "type")]
    pub app_type: AppType,
    /// Target platforms. Absent → `effective_platforms()` returns the default for the type.
    #[serde(default)]
    pub platforms: Vec<Platform>,
    /// Desktop window config (only used when `app_type == Desktop`).
    pub desktop: Option<DesktopConfig>,
    /// Package version string (only used when `app_type == Package`).
    pub version: Option<String>,
}

impl ModuleConfig {
    /// Returns the effective platform list.
    /// If `platforms` is explicitly set in module.json, returns those.
    /// Otherwise returns the type-specific default.
    pub fn effective_platforms(&self) -> Vec<Platform> {
        if !self.platforms.is_empty() {
            return self.platforms.clone();
        }
        match self.app_type {
            AppType::Web                    => vec![Platform::Browser],
            AppType::Backend | AppType::Cli => vec![Platform::Native],
            AppType::Desktop                => vec![Platform::Macos],
            AppType::Package                => vec![],
        }
    }
}
```

- [ ] **Step 1.5 : Ajouter les path helpers pour `.nexa/` dans `NexaProject`**

Ajouter ces méthodes dans l'`impl NexaProject` existant (après `nexa_cache_dir`) :

```rust
    /// Returns `<root>/.nexa/nex_out/<module>/<platform>/` — intermediate Rust sources.
    pub fn nex_out_dir(&self, module: &str, platform: &Platform) -> PathBuf {
        self.root
            .join(".nexa")
            .join("nex_out")
            .join(module)
            .join(platform.as_str())
    }

    /// Returns `<root>/.nexa/compile/logs/<module>-<platform>.log` — compile log path.
    pub fn compile_log_path(&self, module: &str, platform: &Platform) -> PathBuf {
        self.root
            .join(".nexa")
            .join("compile")
            .join("logs")
            .join(format!("{}-{}.log", module, platform.as_str()))
    }

    /// Returns `<root>/dist/<module>/<platform>/` — per-platform build output.
    pub fn dist_platform_dir(&self, module: &str, platform: &Platform) -> PathBuf {
        self.root
            .join("dist")
            .join(module)
            .join(platform.as_str())
    }
```

- [ ] **Step 1.6 : Lancer les tests — vérifier qu'ils passent**

```bash
cargo test -p nexa --lib -- project::tests 2>&1 | tail -20
```

Expected: tous les tests project::tests passent (anciens + nouveaux).

- [ ] **Step 1.7 : Commit**

```bash
git add crates/cli/src/application/project.rs
git commit -m "feat(project): add AppType, Platform, DesktopConfig, effective_platforms()"
```

---

## Task 2 — Ajouter `compile_to_ir` dans `compiler/src/lib.rs`

**Files:**
- Modify: `crates/compiler/src/lib.rs`

- [ ] **Step 2.1 : Écrire le test**

Ajouter dans le bloc `#[cfg(test)]` de `crates/compiler/src/lib.rs` :

```rust
    #[test]
    fn compile_to_ir_extracts_class_from_backend_app() {
        let src = r#"app MyCli {
    main() => Void {
        let x: Int = 42;
        return;
    }
}"#;
        let tmp = tempfile::tempdir().unwrap();
        let entry = tmp.path().join("app.nx");
        std::fs::write(&entry, src).unwrap();
        let ir = compile_to_ir(&entry, tmp.path(), tmp.path(), "core").unwrap();
        assert_eq!(ir.name, "MyCli");
        assert!(!ir.classes.is_empty(), "should have at least one class");
        let cls = &ir.classes[0];
        assert_eq!(cls.name, "MyCli");
        assert!(cls.methods.iter().any(|m| m.name == "main"), "should have main() method");
    }
```

- [ ] **Step 2.2 : Lancer le test — vérifier qu'il échoue**

```bash
cargo test -p nexa-compiler --lib -- tests::compile_to_ir 2>&1 | tail -10
```

Expected: FAIL — `compile_to_ir` not defined.

- [ ] **Step 2.3 : Implémenter `compile_to_ir` dans `lib.rs`**

Ajouter juste avant la fonction `compile_str` :

```rust
/// Compile a `.nx` file in a structured project to an [`IrModule`].
///
/// Pipeline: Lex → Parse → Resolve → SemanticAnalyzer → Lower (IR).
/// The IR is target-agnostic — use it to drive any backend (Rust, WASM, etc.).
#[allow(clippy::result_large_err)]
pub fn compile_to_ir(
    entry: &Path,
    src_root: &Path,
    project_root: &Path,
    module_name: &str,
) -> Result<domain::ir::IrModule, CompileError> {
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

    Ok(application::services::lower::lower(&resolved))
}
```

Ajouter aussi dans le `pub use` en haut de `lib.rs` :

```rust
pub use domain::ir::IrModule;
```

- [ ] **Step 2.4 : Lancer le test**

```bash
cargo test -p nexa-compiler --lib -- tests::compile_to_ir 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 2.5 : Commit**

```bash
git add crates/compiler/src/lib.rs
git commit -m "feat(compiler): expose compile_to_ir and IrModule"
```

---

## Task 3 — Créer `codegen_rust.rs` — IR → Rust

**Files:**
- Create: `crates/compiler/src/application/services/codegen_rust.rs`
- Modify: `crates/compiler/src/application/services/mod.rs`

- [ ] **Step 3.1 : Déclarer le module**

Dans `crates/compiler/src/application/services/mod.rs`, ajouter :

```rust
pub mod codegen_rust;
```

- [ ] **Step 3.2 : Écrire les tests dans `codegen_rust.rs`**

Créer le fichier avec uniquement les tests d'abord :

```rust
//! IR → Rust source transpiler.
//!
//! Converts an [`IrModule`] from a `backend` or `cli` app into:
//! - `main.rs` : compilable Rust source
//! - `Cargo.toml` : workspace-independent manifest with required crate deps

use crate::domain::ir::{
    IrBinOp, IrClass, IrExpr, IrMethod, IrModule, IrParam, IrStmt, IrType, IrUnOp,
};
use std::fmt::Write as FmtWrite;

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RustCodegenError {
    #[error("no app class found in module '{0}'")]
    NoAppClass(String),
    #[error("no main() method found in app class '{0}'")]
    NoMainMethod(String),
    #[error("internal fmt error: {0}")]
    Fmt(#[from] std::fmt::Error),
}

pub struct RustCodegenResult {
    /// Complete `main.rs` source.
    pub main_rs: String,
    /// Complete `Cargo.toml` contents.
    pub cargo_toml: String,
}

pub struct RustCodegen<'a> {
    /// Nexa module name (used in Cargo.toml [package]).
    module_name: &'a str,
    /// Project name (used in Cargo.toml [package]).
    project_name: &'a str,
    /// Project version (used in Cargo.toml [package]).
    project_version: &'a str,
}

impl<'a> RustCodegen<'a> {
    pub fn new(module_name: &'a str, project_name: &'a str, project_version: &'a str) -> Self {
        Self { module_name, project_name, project_version }
    }

    pub fn generate(&self, ir: &IrModule) -> Result<RustCodegenResult, RustCodegenError> {
        let app_class = ir
            .classes
            .iter()
            .find(|c| c.name == ir.name)
            .ok_or_else(|| RustCodegenError::NoAppClass(ir.name.clone()))?;

        let main_method = app_class
            .methods
            .iter()
            .find(|m| m.name == "main")
            .ok_or_else(|| RustCodegenError::NoMainMethod(app_class.name.clone()))?;

        let is_async = main_method.is_async;
        let mut out = String::new();

        // Emit helper structs for non-app classes
        for cls in ir.classes.iter().filter(|c| c.name != ir.name) {
            emit_class(&mut out, cls)?;
        }

        // Emit enums
        for en in &ir.enums {
            writeln!(out, "#[derive(Debug, Clone, PartialEq)]")?;
            writeln!(out, "pub enum {} {{", en.name)?;
            for variant in &en.variants {
                if variant.field_count == 0 {
                    writeln!(out, "    {},", variant.name)?;
                } else {
                    let fields = (0..variant.field_count).map(|_| "i64").collect::<Vec<_>>().join(", ");
                    writeln!(out, "    {}({}),", variant.name, fields)?;
                }
            }
            writeln!(out, "}}")?;
            writeln!(out)?;
        }

        // Emit main function
        if is_async {
            writeln!(out, "#[tokio::main]")?;
            writeln!(out, "async fn main() {{")?;
        } else {
            writeln!(out, "fn main() {{")?;
        }
        let mut body_out = String::new();
        emit_stmts(&mut body_out, &main_method.body, 1)?;
        out.push_str(&body_out);
        writeln!(out, "}}")?;

        let needs_tokio = is_async || uses_tokio(ir);
        let cargo_toml = self.generate_cargo_toml(needs_tokio);

        Ok(RustCodegenResult { main_rs: out, cargo_toml })
    }

    fn generate_cargo_toml(&self, needs_tokio: bool) -> String {
        let mut s = format!(
            r#"[package]
name = "{}-{}"
version = "{}"
edition = "2021"

[[bin]]
name = "{}"
path = "src/main.rs"

[dependencies]
"#,
            self.project_name,
            self.module_name,
            self.project_version,
            self.module_name,
        );
        if needs_tokio {
            s.push_str("tokio = { version = \"1\", features = [\"full\"] }\n");
        }
        s
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn ir_type_to_rust(ty: &IrType) -> String {
    match ty {
        IrType::Int     => "i64".to_string(),
        IrType::Bool    => "bool".to_string(),
        IrType::String  => "String".to_string(),
        IrType::Void    => "()".to_string(),
        IrType::List(t) => format!("Vec<{}>", ir_type_to_rust(t)),
        IrType::Fn(params, ret) => {
            let p = params.iter().map(ir_type_to_rust).collect::<Vec<_>>().join(", ");
            format!("impl Fn({}) -> {}", p, ir_type_to_rust(ret))
        }
        IrType::Named(n) => n.clone(),
        IrType::Unknown  => "_".to_string(),
    }
}

fn ir_binop_to_rust(op: &IrBinOp) -> &'static str {
    match op {
        IrBinOp::Add => "+",  IrBinOp::Sub => "-",
        IrBinOp::Mul => "*",  IrBinOp::Div => "/",
        IrBinOp::Mod => "%",  IrBinOp::Eq  => "==",
        IrBinOp::Ne  => "!=", IrBinOp::Lt  => "<",
        IrBinOp::Gt  => ">",  IrBinOp::Le  => "<=",
        IrBinOp::Ge  => ">=", IrBinOp::And => "&&",
        IrBinOp::Or  => "||",
    }
}

/// Returns `true` if the IR uses any known async stdlib types (HttpServer, Socket…).
fn uses_tokio(ir: &IrModule) -> bool {
    fn expr_uses_tokio(e: &IrExpr) -> bool {
        match e {
            IrExpr::Invoke { callee, .. } => {
                matches!(callee.as_str(), "HttpServer" | "Socket")
            }
            IrExpr::Call { receiver, args, .. } => {
                expr_uses_tokio(receiver) || args.iter().any(expr_uses_tokio)
            }
            IrExpr::Closure { body, .. } => expr_uses_tokio(body),
            IrExpr::Await(_) => true,
            _ => false,
        }
    }
    fn stmts_use_tokio(stmts: &[IrStmt]) -> bool {
        stmts.iter().any(|s| match s {
            IrStmt::Let { init, .. } => expr_uses_tokio(init),
            IrStmt::Discard(e) | IrStmt::Return(Some(e)) | IrStmt::Assign { value: e, .. } => expr_uses_tokio(e),
            IrStmt::If { cond, then_body, else_body } => {
                expr_uses_tokio(cond)
                    || stmts_use_tokio(then_body)
                    || else_body.as_deref().map(stmts_use_tokio).unwrap_or(false)
            }
            IrStmt::While { cond, body } | IrStmt::For { iter: cond, body, .. } => {
                expr_uses_tokio(cond) || stmts_use_tokio(body)
            }
            _ => false,
        })
    }
    ir.classes.iter().any(|c| c.methods.iter().any(|m| stmts_use_tokio(&m.body)))
}

fn emit_class(out: &mut String, cls: &IrClass) -> Result<(), RustCodegenError> {
    writeln!(out, "struct {} {{", cls.name)?;
    for field in &cls.fields {
        writeln!(out, "    {}: {},", field.name, ir_type_to_rust(&field.ty))?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_stmts(out: &mut String, stmts: &[IrStmt], indent: usize) -> Result<(), RustCodegenError> {
    let pad = "    ".repeat(indent);
    for stmt in stmts {
        emit_stmt(out, stmt, indent, &pad)?;
    }
    Ok(())
}

fn emit_stmt(
    out: &mut String,
    stmt: &IrStmt,
    indent: usize,
    pad: &str,
) -> Result<(), RustCodegenError> {
    match stmt {
        IrStmt::Let { name, ty, init } => {
            // Drop Console / stdlib marker instantiations silently.
            if let IrExpr::Invoke { callee, .. } = init {
                if is_stdlib_marker(callee) {
                    return Ok(());
                }
            }
            let rust_ty = ir_type_to_rust(ty);
            let rust_init = emit_expr(init)?;
            writeln!(out, "{pad}let {name}: {rust_ty} = {rust_init};")?;
        }

        IrStmt::Assign { target, value } => {
            writeln!(out, "{pad}{} = {};", emit_expr(target)?, emit_expr(value)?)?;
        }

        IrStmt::Return(Some(e)) => {
            // Suppress void returns (return;)
            if !matches!(e, IrExpr::Invoke { callee, .. } if callee == "Void") {
                writeln!(out, "{pad}return {};", emit_expr(e)?)?;
            }
        }
        IrStmt::Return(None) => {
            writeln!(out, "{pad}return;")?;
        }

        IrStmt::Discard(e) => {
            if let Some(line) = emit_expr_stmt(e)? {
                writeln!(out, "{pad}{line};")?;
            }
        }

        IrStmt::If { cond, then_body, else_body } => {
            writeln!(out, "{pad}if {} {{", emit_expr(cond)?)?;
            emit_stmts(out, then_body, indent + 1)?;
            if let Some(eb) = else_body {
                writeln!(out, "{pad}}} else {{")?;
                emit_stmts(out, eb, indent + 1)?;
            }
            writeln!(out, "{pad}}}")?;
        }

        IrStmt::While { cond, body } => {
            writeln!(out, "{pad}while {} {{", emit_expr(cond)?)?;
            emit_stmts(out, body, indent + 1)?;
            writeln!(out, "{pad}}}")?;
        }

        IrStmt::For { var, iter, body } => {
            writeln!(out, "{pad}for {var} in {} {{", emit_expr(iter)?)?;
            emit_stmts(out, body, indent + 1)?;
            writeln!(out, "{pad}}}")?;
        }

        IrStmt::Break => { writeln!(out, "{pad}break;")?; }
        IrStmt::Continue => { writeln!(out, "{pad}continue;")?; }

        IrStmt::Match { subject_var, subject, arms } => {
            writeln!(out, "{pad}let {subject_var} = {};", emit_expr(subject)?)?;
            writeln!(out, "{pad}match {subject_var} {{")?;
            let inner_pad = "    ".repeat(indent + 1);
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    writeln!(out, "{inner_pad}{} => {{", emit_expr(cond)?)?;
                } else {
                    writeln!(out, "{inner_pad}_ => {{")?;
                }
                emit_stmts(out, &arm.body, indent + 2)?;
                writeln!(out, "{inner_pad}}}")?;
            }
            writeln!(out, "{pad}}}")?;
        }
    }
    Ok(())
}

/// Emit an expression as a *statement* line, handling stdlib calls specially.
/// Returns `None` if the expression should be silently dropped (e.g. void calls to markers).
fn emit_expr_stmt(e: &IrExpr) -> Result<Option<String>, RustCodegenError> {
    match e {
        // Console.log / Console.info / Console.warn / Console.error → println!
        IrExpr::Call { method, args, .. }
            if matches!(method.as_str(), "log" | "info" | "warn" | "error" | "debug") =>
        {
            let arg = args.first().map(|a| emit_expr(a)).transpose()?.unwrap_or_default();
            Ok(Some(format!("println!(\"{{}}\", {arg})")))
        }
        // Await: emit the inner expression
        IrExpr::Await(inner) => Ok(Some(format!("{}.await", emit_expr(inner)?))),
        // Generic call
        _ => Ok(Some(emit_expr(e)?)),
    }
}

fn emit_expr(e: &IrExpr) -> Result<String, RustCodegenError> {
    match e {
        IrExpr::Int(n) => Ok(n.to_string()),
        IrExpr::Bool(b) => Ok(b.to_string()),
        IrExpr::Str(s) => Ok(format!("\"{s}\".to_string()")),
        IrExpr::Local(n) => Ok(n.clone()),
        IrExpr::SelfRef => Ok("self".to_string()),

        IrExpr::Field { receiver, name } => {
            Ok(format!("{}.{name}", emit_expr(receiver)?))
        }

        IrExpr::Call { receiver, method, args } => {
            // Console.log / etc. — handled in emit_expr_stmt; fall through here for nesting
            let recv = emit_expr(receiver)?;
            let a = args.iter().map(emit_expr).collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{recv}.{method}({})", a.join(", ")))
        }

        IrExpr::Invoke { callee, args } => {
            let a = args.iter().map(emit_expr).collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{callee}({})", a.join(", ")))
        }

        IrExpr::Bin { op, lhs, rhs } => {
            Ok(format!("({} {} {})", emit_expr(lhs)?, ir_binop_to_rust(op), emit_expr(rhs)?))
        }

        IrExpr::Unary { op, operand } => {
            let op_str = match op { IrUnOp::Not => "!", IrUnOp::Neg => "-" };
            Ok(format!("{op_str}{}", emit_expr(operand)?))
        }

        IrExpr::Await(inner) => Ok(format!("{}.await", emit_expr(inner)?)),

        IrExpr::List(items) => {
            let elems = items.iter().map(emit_expr).collect::<Result<Vec<_>, _>>()?;
            Ok(format!("vec![{}]", elems.join(", ")))
        }

        IrExpr::Closure { params, body } => {
            let ps = params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
            Ok(format!("|{ps}| {}", emit_expr(body)?))
        }

        // Node (UI) and DynamicImport have no Rust equivalent in Phase 1
        IrExpr::Node { tag, .. } => Ok(format!("/* UI node: {tag} */")),
        IrExpr::DynamicImport(p) => Ok(format!("/* import(\"{p}\") */")),
    }
}

/// Returns true if `callee` is a stdlib type that should be silently dropped
/// as a variable binding target (e.g. Console, because its methods map directly
/// to println! without needing an instance).
fn is_stdlib_marker(callee: &str) -> bool {
    matches!(callee, "Console")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ir::*;

    fn make_gen() -> RustCodegen<'static> {
        RustCodegen::new("core", "my-app", "0.1.0")
    }

    fn make_ir(name: &str, stmts: Vec<IrStmt>) -> IrModule {
        IrModule {
            name: name.to_string(),
            server: None,
            enums: vec![],
            classes: vec![IrClass {
                name: name.to_string(),
                kind: IrClassKind::Class,
                is_public: false,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![IrMethod {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: IrType::Void,
                    body: stmts,
                    is_public: false,
                    is_async: false,
                }],
            }],
            routes: vec![],
        }
    }

    #[test]
    fn generates_fn_main_for_empty_app() {
        let ir = make_ir("MyCli", vec![]);
        let result = make_gen().generate(&ir).unwrap();
        assert!(result.main_rs.contains("fn main()"), "should emit fn main()");
        assert!(!result.main_rs.contains("async"), "should not be async");
    }

    #[test]
    fn generates_tokio_main_for_async_app() {
        let ir = IrModule {
            name: "MyApi".to_string(),
            server: None,
            enums: vec![],
            classes: vec![IrClass {
                name: "MyApi".to_string(),
                kind: IrClassKind::Class,
                is_public: false,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![IrMethod {
                    name: "main".to_string(),
                    params: vec![],
                    return_ty: IrType::Void,
                    body: vec![],
                    is_public: false,
                    is_async: true,
                }],
            }],
            routes: vec![],
        };
        let result = make_gen().generate(&ir).unwrap();
        assert!(result.main_rs.contains("#[tokio::main]"), "should emit #[tokio::main]");
        assert!(result.main_rs.contains("async fn main()"), "should emit async fn main()");
        assert!(result.cargo_toml.contains("tokio"), "Cargo.toml should include tokio");
    }

    #[test]
    fn let_int_binding_emits_i64() {
        let stmts = vec![IrStmt::Let {
            name: "x".to_string(),
            ty: IrType::Int,
            init: IrExpr::Int(42),
        }];
        let ir = make_ir("App", stmts);
        let result = make_gen().generate(&ir).unwrap();
        assert!(result.main_rs.contains("let x: i64 = 42;"), "got:\n{}", result.main_rs);
    }

    #[test]
    fn let_string_binding_emits_to_string() {
        let stmts = vec![IrStmt::Let {
            name: "s".to_string(),
            ty: IrType::String,
            init: IrExpr::Str("hello".to_string()),
        }];
        let ir = make_ir("App", stmts);
        let result = make_gen().generate(&ir).unwrap();
        assert!(
            result.main_rs.contains("let s: String = \"hello\".to_string();"),
            "got:\n{}",
            result.main_rs
        );
    }

    #[test]
    fn console_log_emits_println() {
        let stmts = vec![
            // let c = Console(); — should be dropped
            IrStmt::Let {
                name: "c".to_string(),
                ty: IrType::Named("Console".to_string()),
                init: IrExpr::Invoke { callee: "Console".to_string(), args: vec![] },
            },
            // c.log("hello"); — should become println!
            IrStmt::Discard(IrExpr::Call {
                receiver: Box::new(IrExpr::Local("c".to_string())),
                method: "log".to_string(),
                args: vec![IrExpr::Str("hello".to_string())],
            }),
        ];
        let ir = make_ir("App", stmts);
        let result = make_gen().generate(&ir).unwrap();
        assert!(
            result.main_rs.contains("println!("),
            "should emit println!, got:\n{}",
            result.main_rs
        );
        assert!(
            !result.main_rs.contains("let c: Console"),
            "Console binding should be dropped, got:\n{}",
            result.main_rs
        );
    }

    #[test]
    fn binary_op_add_emits_plus() {
        let stmts = vec![IrStmt::Let {
            name: "r".to_string(),
            ty: IrType::Int,
            init: IrExpr::Bin {
                op: IrBinOp::Add,
                lhs: Box::new(IrExpr::Int(1)),
                rhs: Box::new(IrExpr::Int(2)),
            },
        }];
        let ir = make_ir("App", stmts);
        let result = make_gen().generate(&ir).unwrap();
        assert!(result.main_rs.contains("(1 + 2)"), "got:\n{}", result.main_rs);
    }

    #[test]
    fn cargo_toml_contains_package_info() {
        let ir = make_ir("App", vec![]);
        let result = make_gen().generate(&ir).unwrap();
        assert!(result.cargo_toml.contains("[package]"));
        assert!(result.cargo_toml.contains("my-app-core"));
        assert!(result.cargo_toml.contains("0.1.0"));
        assert!(result.cargo_toml.contains("[[bin]]"));
    }

    #[test]
    fn error_when_no_app_class() {
        let ir = IrModule {
            name: "Ghost".to_string(),
            server: None,
            enums: vec![],
            classes: vec![],
            routes: vec![],
        };
        let err = make_gen().generate(&ir).unwrap_err();
        assert!(matches!(err, RustCodegenError::NoAppClass(_)));
    }

    #[test]
    fn error_when_no_main_method() {
        let ir = IrModule {
            name: "App".to_string(),
            server: None,
            enums: vec![],
            classes: vec![IrClass {
                name: "App".to_string(),
                kind: IrClassKind::Class,
                is_public: false,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![],
            }],
            routes: vec![],
        };
        let err = make_gen().generate(&ir).unwrap_err();
        assert!(matches!(err, RustCodegenError::NoMainMethod(_)));
    }
}
```

- [ ] **Step 3.3 : Lancer les tests — vérifier qu'ils échouent**

```bash
cargo test -p nexa-compiler --lib -- codegen_rust::tests 2>&1 | tail -20
```

Expected: FAIL — module not complete yet (tests are in the file but the impls are too, that's fine for TDD here since we write impl + tests together).

- [ ] **Step 3.4 : Lancer les tests — vérifier qu'ils passent**

```bash
cargo test -p nexa-compiler --lib -- codegen_rust::tests 2>&1 | tail -30
```

Expected: tous les tests `codegen_rust::tests` passent.

- [ ] **Step 3.5 : Exposer `RustCodegen` dans `lib.rs`**

Ajouter dans les `pub use` en haut de `crates/compiler/src/lib.rs` :

```rust
pub use application::services::codegen_rust::{RustCodegen, RustCodegenError, RustCodegenResult};
```

- [ ] **Step 3.6 : Commit**

```bash
git add crates/compiler/src/application/services/codegen_rust.rs \
        crates/compiler/src/application/services/mod.rs \
        crates/compiler/src/lib.rs
git commit -m "feat(compiler): add codegen_rust — IR to Rust source transpiler"
```

---

## Task 4 — Créer le module `targets/`

**Files:**
- Create: `crates/cli/src/application/targets/mod.rs`
- Create: `crates/cli/src/application/targets/web.rs`
- Create: `crates/cli/src/application/targets/rust.rs`
- Create: `crates/cli/src/application/targets/desktop.rs`
- Create: `crates/cli/src/application/targets/dispatcher.rs`
- Modify: `crates/cli/src/application/mod.rs`
- Modify: `crates/cli/Cargo.toml`

- [ ] **Step 4.1 : Ajouter rayon dans Cargo.toml**

Dans `crates/cli/Cargo.toml`, ajouter dans `[dependencies]` :

```toml
rayon = "1"
```

- [ ] **Step 4.2 : Créer `targets/mod.rs`**

```rust
//! Build targets — one sub-module per compilation target.
//!
//! Each target receives a compiled `IrModule` (already lowered from AST)
//! and is responsible for producing the final artefact in `dist/<module>/<platform>/`.

pub mod desktop;
pub mod dispatcher;
pub mod rust;
pub mod web;
```

- [ ] **Step 4.3 : Déclarer `targets` dans `application/mod.rs`**

Dans `crates/cli/src/application/mod.rs`, ajouter :

```rust
pub mod targets;
```

- [ ] **Step 4.4 : Créer `targets/web.rs`**

```rust
//! Web target — compiles Nexa → HTML+JS → dist/<module>/browser/

use crate::application::project::NexaProject;
use crate::infrastructure::ui;
use nexa_compiler::compile_project_file;
use std::fs;
use std::path::Path;

/// Build a web module into `dist/<module>/browser/`.
pub fn build(proj: &NexaProject, mod_name: &str, log_path: &Path) {
    let out_dir = proj.dist_platform_dir(mod_name, &crate::application::project::Platform::Browser);
    fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| ui::die(format!("cannot create web output dir: {e}")));

    let log_dir = log_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(log_dir)
        .unwrap_or_else(|e| ui::die(format!("cannot create log dir: {e}")));

    match compile_project_file(
        &proj.module_entry(mod_name),
        &proj.module_src_root(mod_name),
        proj.root(),
        mod_name,
    ) {
        Ok(result) => {
            fs::write(out_dir.join("index.html"), &result.html)
                .unwrap_or_else(|e| ui::die(format!("cannot write index.html: {e}")));
            fs::write(out_dir.join("app.js"), &result.js)
                .unwrap_or_else(|e| ui::die(format!("cannot write app.js: {e}")));
            let log = format!("[OK] web/browser — {} bytes html, {} bytes js\n", result.html.len(), result.js.len());
            let _ = fs::write(log_path, &log);
        }
        Err(e) => {
            let _ = fs::write(log_path, format!("[ERROR] {e}\n"));
            ui::die(e.to_string());
        }
    }
}
```

- [ ] **Step 4.5 : Créer `targets/desktop.rs`** (stub Phase 1)

```rust
//! Desktop target — stub for Phase 1, implemented in Phase 2.

use crate::infrastructure::ui;
use std::path::Path;

pub fn build(_proj: &crate::application::project::NexaProject, mod_name: &str, _log_path: &Path) {
    ui::die(format!(
        "module '{}': desktop builds not yet supported (planned for Phase 2)",
        mod_name
    ));
}
```

- [ ] **Step 4.6 : Créer `targets/rust.rs`**

```rust
//! Rust native target — compiles Nexa → Rust source → native binary via cargo.
//!
//! Output structure:
//!   .nexa/nex_out/<module>/<platform>/src/main.rs   ← generated Rust
//!   .nexa/nex_out/<module>/<platform>/Cargo.toml    ← generated manifest
//!   dist/<module>/<platform>/<binary>               ← compiled binary

use crate::application::project::{NexaProject, Platform};
use crate::infrastructure::ui;
use nexa_compiler::{compile_to_ir, RustCodegen};
use std::{fs, path::Path, process::Command};

/// Build a backend/cli module for the given platform.
pub fn build(proj: &NexaProject, mod_name: &str, platform: &Platform, log_path: &Path) {
    // Ensure log dir exists
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| ui::die(format!("cannot create log dir: {e}")));
    }

    // 1. Compile Nexa → IR
    let ir = match compile_to_ir(
        &proj.module_entry(mod_name),
        &proj.module_src_root(mod_name),
        proj.root(),
        mod_name,
    ) {
        Ok(ir) => ir,
        Err(e) => {
            let _ = fs::write(log_path, format!("[ERROR] IR compilation: {e}\n"));
            ui::die(e.to_string());
        }
    };

    // 2. IR → Rust source
    let codegen = RustCodegen::new(mod_name, &proj.project.name, &proj.project.version);
    let rust_output = match codegen.generate(&ir) {
        Ok(r) => r,
        Err(e) => {
            let _ = fs::write(log_path, format!("[ERROR] Rust codegen: {e}\n"));
            ui::die(e.to_string());
        }
    };

    // 3. Write intermediate source to .nexa/nex_out/<module>/<platform>/
    let nex_out = proj.nex_out_dir(mod_name, platform);
    let src_dir = nex_out.join("src");
    fs::create_dir_all(&src_dir)
        .unwrap_or_else(|e| ui::die(format!("cannot create nex_out src dir: {e}")));
    fs::write(src_dir.join("main.rs"), &rust_output.main_rs)
        .unwrap_or_else(|e| ui::die(format!("cannot write main.rs: {e}")));
    fs::write(nex_out.join("Cargo.toml"), &rust_output.cargo_toml)
        .unwrap_or_else(|e| ui::die(format!("cannot write Cargo.toml: {e}")));

    // 4. cargo build --release in the nex_out dir
    let dist_dir = proj.dist_platform_dir(mod_name, platform);
    fs::create_dir_all(&dist_dir)
        .unwrap_or_else(|e| ui::die(format!("cannot create dist dir: {e}")));

    let rust_target = platform_to_rust_target(platform);
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--release");
    if let Some(target) = rust_target {
        cmd.arg("--target").arg(target);
    }
    cmd.current_dir(&nex_out);

    let output = cmd.output().unwrap_or_else(|e| {
        ui::die(format!("cannot run cargo: {e} — is Rust installed?"));
    });

    let log_content = format!(
        "[cargo build]\nstdout:\n{}\nstderr:\n{}\nexit: {}\n",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        output.status,
    );
    let _ = fs::write(log_path, &log_content);

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        ui::die(format!("cargo build failed for module '{mod_name}' ({platform:?})"));
    }

    // 5. Copy binary to dist/
    let binary_name = if cfg!(target_os = "windows") {
        format!("{mod_name}.exe")
    } else {
        mod_name.to_string()
    };

    let target_subdir = rust_target.unwrap_or("release");
    let built = nex_out
        .join("target")
        .join(target_subdir)
        .join("release")
        .join(&binary_name);

    // Fallback: no cross-compile target → target/release/
    let built = if built.exists() {
        built
    } else {
        nex_out.join("target").join("release").join(&binary_name)
    };

    if built.exists() {
        fs::copy(&built, dist_dir.join(&binary_name))
            .unwrap_or_else(|e| ui::die(format!("cannot copy binary: {e}")));
    }
}

fn platform_to_rust_target(platform: &Platform) -> Option<&'static str> {
    match platform {
        Platform::NativeMacos   => Some("aarch64-apple-darwin"),
        Platform::NativeWindows => Some("x86_64-pc-windows-gnu"),
        Platform::NativeLinux   => Some("x86_64-unknown-linux-gnu"),
        Platform::Native        => None, // build for current host
        _ => None,
    }
}
```

- [ ] **Step 4.7 : Créer `targets/dispatcher.rs`**

```rust
//! Build dispatcher — iterates over effective platforms in parallel (Rayon)
//! and calls the right target for each (module_type × platform) combination.

use crate::application::{
    project::{AppType, NexaProject, Platform},
    targets::{desktop, rust, web},
};
use crate::infrastructure::ui;
use rayon::prelude::*;

/// Compile one module across all its effective platforms.
/// The source is compiled to IR once (inside each target), parallelised by Rayon.
pub fn build_module(proj: &NexaProject, mod_name: &str) {
    let module_cfg = proj
        .modules
        .get(mod_name)
        .unwrap_or_else(|| ui::die(format!("module '{mod_name}' not found in project")));

    let platforms = module_cfg.effective_platforms();

    if platforms.is_empty() {
        // type: package — nothing to build, handled by `nexa package`
        return;
    }

    platforms.par_iter().for_each(|platform| {
        let log_path = proj.compile_log_path(mod_name, platform);
        match &module_cfg.app_type {
            AppType::Web | AppType::Package => {
                web::build(proj, mod_name, &log_path);
            }
            AppType::Backend | AppType::Cli => {
                rust::build(proj, mod_name, platform, &log_path);
            }
            AppType::Desktop => {
                desktop::build(proj, mod_name, &log_path);
            }
        }
    });
}
```

- [ ] **Step 4.8 : Compiler pour vérifier qu'il n'y a pas d'erreurs**

```bash
cargo build -p nexa 2>&1 | tail -30
```

Expected: compile OK (warnings possibles, pas d'erreurs).

- [ ] **Step 4.9 : Commit**

```bash
git add crates/cli/src/application/targets/ \
        crates/cli/src/application/mod.rs \
        crates/cli/Cargo.toml
git commit -m "feat(cli): add targets/ module — web, rust, desktop stubs, dispatcher"
```

---

## Task 5 — Refactoriser `build.rs` pour utiliser le dispatcher

**Files:**
- Modify: `crates/cli/src/application/commands/build.rs`

- [ ] **Step 5.1 : Remplacer la boucle de build dans `build()`**

Remplacer la boucle `for mod_name in &modules { ... match compile_project_file(...) ... }` par un appel au dispatcher. Voici le remplacement de la fonction `build()` entière :

```rust
pub fn build(project_dir: Option<PathBuf>) {
    updater::check_and_notify("stable");
    let proj = load_project(project_dir);
    let modules = proj
        .compiler
        .active_modules(&proj.project.modules)
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    let existing_lock = load_build_lock(proj.root());

    let mut lock_entries: Vec<(String, Vec<BuildLockEntry>)> = Vec::new();
    let mut compiled = 0usize;
    let mut skipped = 0usize;

    let pb = ui::progress_bar("Compiling…", modules.len() as u64);

    for mod_name in &modules {
        let current_sources =
            fingerprint_module_sources(&proj.module_src_root(mod_name), proj.root());

        // Use all effective platform dist dirs for up-to-date check
        let module_cfg = proj.modules.get(mod_name.as_str())
            .unwrap_or_else(|| ui::die(format!("module '{mod_name}' missing from project")));
        let platforms = module_cfg.effective_platforms();
        let any_dist_dir = platforms
            .first()
            .map(|p| proj.dist_platform_dir(mod_name, p))
            .unwrap_or_else(|| proj.dist_dir(mod_name));

        if is_module_up_to_date(&existing_lock, mod_name, &current_sources, &any_dist_dir) {
            lock_entries.push((mod_name.clone(), current_sources));
            skipped += 1;
            pb.inc(1);
            continue;
        }

        crate::application::targets::dispatcher::build_module(&proj, mod_name);
        lock_entries.push((mod_name.clone(), current_sources));
        compiled += 1;
        pb.inc(1);
    }

    let refs: Vec<(&str, Vec<BuildLockEntry>)> =
        lock_entries.iter().map(|(n, e)| (n.as_str(), e.clone())).collect();
    save_build_lock(proj.root(), &refs);

    let summary = match (compiled, skipped) {
        (c, 0) => format!("Build OK — {c} module(s) compiled"),
        (0, s) => format!("Build OK — {s} module(s) up to date (nothing to compile)"),
        (c, s) => format!("Build OK — {c} compiled, {s} up to date"),
    };
    ui::bar_done(&pb, summary);
}
```

- [ ] **Step 5.2 : Supprimer l'import `compile_project_file` devenu inutile dans build.rs**

Retirer `compile_project_file` de la ligne `use nexa_compiler::{...}` si elle n'est plus utilisée ailleurs dans le fichier. Garder `compile_to_bundle`, `decode_nxb`, `CodeGenerator` (utilisés par `package()` et `run_from_bundle()`).

- [ ] **Step 5.3 : Vérifier que ça compile**

```bash
cargo build -p nexa 2>&1 | tail -20
```

Expected: compile OK.

- [ ] **Step 5.4 : Lancer tous les tests existants**

```bash
cargo test -p nexa --lib 2>&1 | tail -20
```

Expected: tous les tests passent (aucun test de la commande `build` ne dépend du chemin de sortie `dist/<module>/` directement).

- [ ] **Step 5.5 : Commit**

```bash
git add crates/cli/src/application/commands/build.rs
git commit -m "refactor(build): delegate to targets/dispatcher — build_module() per module"
```

---

## Task 6 — Mettre à jour les commandes CLI — `module.rs`, `init.rs`, `cli.rs`

**Files:**
- Modify: `crates/cli/src/application/commands/module.rs`
- Modify: `crates/cli/src/application/commands/init.rs`
- Modify: `crates/cli/src/interfaces/cli.rs`
- Modify: `crates/cli/src/application/commands/mod.rs`

- [ ] **Step 6.1 : Écrire les tests pour module_add avec --type**

Ajouter dans le bloc `#[cfg(test)]` de `module.rs` :

```rust
    #[test]
    fn module_add_with_type_backend_writes_type_in_module_json() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            Some("backend".to_string()),
            None,
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("api").join("module.json"),
        ).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("backend"));
    }

    #[test]
    fn module_add_with_type_and_platforms_writes_both() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add(
            "api".to_string(),
            Some(tmp.path().to_path_buf()),
            Some("backend".to_string()),
            Some("native-linux,native-macos".to_string()),
        );

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("api").join("module.json"),
        ).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("backend"));
        let plats: Vec<&str> = val["platforms"].as_array().unwrap()
            .iter().filter_map(|p| p.as_str()).collect();
        assert!(plats.contains(&"native-linux"));
        assert!(plats.contains(&"native-macos"));
    }

    #[test]
    fn module_add_without_type_omits_type_field() {
        let tmp = TempDir::new().unwrap();
        make_project(tmp.path());

        module_add("web-module".to_string(), Some(tmp.path().to_path_buf()), None, None);

        let raw = fs::read_to_string(
            tmp.path().join("modules").join("web-module").join("module.json"),
        ).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // No "type" field → defaults to web at runtime
        assert!(val.get("type").is_none() || val["type"].as_str() == Some("web"));
    }
```

- [ ] **Step 6.2 : Lancer — vérifier FAIL**

```bash
cargo test -p nexa --lib -- module::tests::module_add_with_type 2>&1 | tail -10
```

Expected: FAIL — `module_add` doesn't accept type/platforms yet.

- [ ] **Step 6.3 : Mettre à jour la signature de `module_add` dans `module.rs`**

Remplacer la fonction `module_add` entière :

```rust
pub fn module_add(
    name: String,
    project_dir: Option<PathBuf>,
    app_type: Option<String>,
    platforms: Option<String>,
) {
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        ui::die(format!(
            "module name '{}' must only contain letters, digits, hyphens or underscores",
            name
        ));
    }

    let proj = load_project(project_dir);

    if proj.project.modules.contains(&name) {
        ui::die(format!("module '{name}' already exists in this project."));
    }

    let root = proj.root().to_path_buf();
    let src_main = root.join("modules").join(&name).join("src").join("main");
    let src_test = root.join("modules").join(&name).join("src").join("test");

    fs::create_dir_all(&src_main)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory: {e}")));
    fs::create_dir_all(&src_test)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory: {e}")));

    // Build module.json with optional type/platforms
    let type_line = if let Some(ref t) = app_type {
        format!(",\n  \"type\": \"{}\"", t)
    } else {
        String::new()
    };
    let platforms_line = if let Some(ref p) = platforms {
        let plat_array: Vec<String> = p.split(',').map(|s| format!("\"{}\"", s.trim())).collect();
        format!(",\n  \"platforms\": [{}]", plat_array.join(", "))
    } else {
        String::new()
    };
    let module_json = format!(
        r#"{{
  "name": "{name}",
  "main": "app.nx",
  "dependencies": {{}}{type_line}{platforms_line}
}}
"#
    );
    write_file(
        &root.join("modules").join(&name).join("module.json"),
        &module_json,
    );

    let app_class = to_pascal_case(&name);
    let app_nx = if app_type.as_deref() == Some("backend") || app_type.as_deref() == Some("cli") {
        format!(
            r#"package {pkg};

app {app} {{
    main() => Void {{
        let c = Console();
        c.log("Hello from {app}!");
    }}
}}
"#,
            pkg = name.replace('-', "_"),
            app = app_class,
        )
    } else {
        format!(
            r#"package {pkg};

app {app} {{
  server {{ port: 3000; }}

  public window HomePage {{
    public render() => Component {{
      return Page {{
        Heading("Module {app}")
      }};
    }}
  }}

  route "/" => HomePage;
}}
"#,
            pkg = name.replace('-', "_"),
            app = app_class,
        )
    };
    write_file(&src_main.join("app.nx"), &app_nx);

    // Add module to project.json
    let proj_path = root.join("project.json");
    if let Ok(text) = fs::read_to_string(&proj_path) {
        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(modules) = val.get_mut("modules").and_then(|m| m.as_array_mut()) {
                modules.push(serde_json::Value::String(name.clone()));
            }
            if let Ok(updated) = serde_json::to_string_pretty(&val) {
                let _ = fs::write(&proj_path, updated);
            }
        }
    }

    ui::blank();
    ui::success(format!("Module \x1b[1m{name}\x1b[0m added"));
    ui::blank();
    ui::hint(format!("  modules/{name}/"));
    ui::hint("  ├── module.json");
    ui::hint("  └── src/main/app.nx");
    ui::blank();
    if app_type.is_none() {
        ui::hint(format!(
            "  Set as main:  nexa-compiler.yaml → main_module: \"{name}\""
        ));
    }
    ui::blank();
}
```

- [ ] **Step 6.4 : Mettre à jour `mod.rs` pour re-exporter avec nouvelle signature**

Dans `crates/cli/src/application/commands/mod.rs`, la ligne `pub use module::module_add;` reste inchangée — la signature change dans le fichier source.

- [ ] **Step 6.5 : Mettre à jour `cli.rs` — ajouter --type et --platforms à `Module::Add`**

Remplacer le bloc `ModuleAction::Add` dans `#[derive(Subcommand)] enum ModuleAction` :

```rust
#[derive(Subcommand)]
enum ModuleAction {
    /// Add a new module to the project
    Add {
        /// Module name (used as the directory name under modules/)
        #[arg(value_name = "NAME")]
        name: String,
        /// Project root directory (default: current directory)
        #[arg(short, long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Application type: web, backend, cli, desktop, package (default: web)
        #[arg(long, value_name = "TYPE")]
        r#type: Option<String>,
        /// Target platforms, comma-separated (e.g. native-linux,native-macos)
        #[arg(long, value_name = "PLATFORMS")]
        platforms: Option<String>,
    },
}
```

Et mettre à jour le dispatch dans `pub async fn run()` :

```rust
        Commands::Module { action } => match action {
            ModuleAction::Add { name, project, r#type, platforms } => {
                commands::module_add(name, project, r#type, platforms)
            }
        },
```

- [ ] **Step 6.6 : Lancer les tests — vérifier qu'ils passent**

```bash
cargo test -p nexa --lib -- module::tests 2>&1 | tail -20
```

Expected: tous les tests module::tests passent.

- [ ] **Step 6.7 : Compiler pour vérifier**

```bash
cargo build -p nexa 2>&1 | tail -10
```

Expected: compile OK.

- [ ] **Step 6.8 : Commit**

```bash
git add crates/cli/src/application/commands/module.rs \
        crates/cli/src/interfaces/cli.rs
git commit -m "feat(cli): add --type and --platforms flags to nexa module add"
```

---

## Task 7 — Mettre à jour `init.rs` et `cli.rs` pour `nexa new --type`

**Files:**
- Modify: `crates/cli/src/application/commands/init.rs`
- Modify: `crates/cli/src/interfaces/cli.rs`

- [ ] **Step 7.1 : Écrire le test**

Ajouter dans le bloc `#[cfg(test)]` de `init.rs` (il n'en a pas — ajouter à la fin du fichier) :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn create_project_files_with_type_backend_writes_module_json() {
        let tmp = TempDir::new().unwrap();
        create_project_files_typed(tmp.path(), "my-api", "Dev", "0.1.0", Some("backend"), None);
        let raw = fs::read_to_string(
            tmp.path().join("modules").join("core").join("module.json"),
        ).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["type"].as_str(), Some("backend"));
    }

    #[test]
    fn create_project_files_without_type_omits_type_field() {
        let tmp = TempDir::new().unwrap();
        create_project_files_typed(tmp.path(), "my-web", "Dev", "0.1.0", None, None);
        let raw = fs::read_to_string(
            tmp.path().join("modules").join("core").join("module.json"),
        ).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(val.get("type").is_none(), "type should be absent for web default");
    }
}
```

- [ ] **Step 7.2 : Lancer — vérifier FAIL**

```bash
cargo test -p nexa --lib -- init::tests 2>&1 | tail -10
```

Expected: FAIL — `create_project_files_typed` not defined.

- [ ] **Step 7.3 : Refactoriser `create_project_files` en `create_project_files_typed`**

Renommer `create_project_files` en `create_project_files_typed` et ajouter les paramètres `app_type: Option<&str>` et `platforms: Option<&str>`. Mettre à jour `init()` pour appeler la nouvelle signature.

```rust
/// Create all project files and directories under `root`.
fn create_project_files_typed(
    root: &Path,
    project_name: &str,
    author: &str,
    version: &str,
    app_type: Option<&str>,
    platforms: Option<&str>,
) {
    let core_src_main = root.join("modules").join("core").join("src").join("main");
    let core_src_test = root.join("modules").join("core").join("src").join("test");
    fs::create_dir_all(&core_src_main)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory structure: {e}")));
    fs::create_dir_all(&core_src_test)
        .unwrap_or_else(|e| ui::die(format!("cannot create directory structure: {e}")));

    let project_json = format!(
        r#"{{
  "name": "{name}",
  "version": "{ver}",
  "author": "{author}",
  "modules": ["core"],
  "dependencies": {{}}
}}
"#,
        name = project_name, ver = version, author = author,
    );
    write_file(&root.join("project.json"), &project_json);

    let type_line = if let Some(t) = app_type {
        format!(",\n  \"type\": \"{}\"", t)
    } else {
        String::new()
    };
    let platforms_line = if let Some(p) = platforms {
        let arr: Vec<String> = p.split(',').map(|s| format!("\"{}\"", s.trim())).collect();
        format!(",\n  \"platforms\": [{}]", arr.join(", "))
    } else {
        String::new()
    };
    let module_json = format!(
        r#"{{
  "name": "core",
  "main": "app.nx",
  "dependencies": {{}}{type_line}{platforms_line}
}}
"#
    );
    write_file(&root.join("modules").join("core").join("module.json"), &module_json);

    // nexa-compiler.yaml (unchanged)
    let compiler_yaml = r#"version: "0.1"
main_module: "core"
# include_modules:
#   - core
# exclude_modules: []
# registry: "https://registry.nexa-lang.org"
# private_registries:
#   - url: "https://corp.registry.example.com"
#     key: "sk_live_..."
"#;
    write_file(&root.join("nexa-compiler.yaml"), compiler_yaml);

    let app_class = to_pascal_case(project_name);
    let app_nx = if app_type == Some("backend") || app_type == Some("cli") {
        format!(
            r#"package {pkg};

app {app} {{
    main() => Void {{
        let c = Console();
        c.log("Hello from {app}!");
    }}
}}
"#,
            pkg = project_name.replace('-', "_"),
            app = app_class,
        )
    } else {
        format!(
            r#"package {pkg};

app {app} {{
  server {{ port: 3000; }}

  public window HomePage {{
    public render() => Component {{
      return Page {{
        Heading("Welcome to {app}!")
      }};
    }}
  }}

  route "/" => HomePage;
}}
"#,
            pkg = project_name.replace('-', "_"),
            app = app_class,
        )
    };
    write_file(&core_src_main.join("app.nx"), &app_nx);

    write_file(
        &root.join(".gitignore"),
        "dist/\n.nexa/nex_out/\n.nexa/compile/logs/\nnode_modules/\n",
    );
}
```

Mettre à jour l'appel dans `init()` :

```rust
    create_project_files_typed(&root, &project_name, &author_str, &version, app_type.as_deref(), None);
```

Et mettre à jour la signature de `init()` :

```rust
pub fn init(name: Option<String>, author: Option<String>, version: String, no_git: bool, app_type: Option<String>) {
```

- [ ] **Step 7.4 : Mettre à jour `cli.rs` — ajouter `--type` à la commande `Init`**

Remplacer le variant `Init` dans l'enum `Commands` :

```rust
    /// Create a new Nexa project in a new directory
    Init {
        /// Project name (also used as the directory name)
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Project author
        #[arg(long, value_name = "AUTHOR")]
        author: Option<String>,
        /// Initial version (default: 0.1.0)
        #[arg(long, value_name = "VERSION", default_value = "0.1.0")]
        version: String,
        /// Do not initialise a git repository
        #[arg(long)]
        no_git: bool,
        /// Application type: web, backend, cli, desktop, package (default: web)
        #[arg(long, value_name = "TYPE")]
        r#type: Option<String>,
    },
```

Et mettre à jour le dispatch :

```rust
        Commands::Init { name, author, version, no_git, r#type } => {
            commands::init(name, author, version, no_git, r#type)
        }
```

- [ ] **Step 7.5 : Lancer les tests**

```bash
cargo test -p nexa --lib -- init::tests 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 7.6 : Compiler**

```bash
cargo build -p nexa 2>&1 | tail -10
```

Expected: OK.

- [ ] **Step 7.7 : Commit**

```bash
git add crates/cli/src/application/commands/init.rs \
        crates/cli/src/interfaces/cli.rs
git commit -m "feat(cli): add --type flag to nexa new — generates typed module.json"
```

---

## Task 8 — Mettre à jour `.gitignore`

**Files:**
- Modify: `.gitignore` (si existant) ou créer à la racine du projet

- [ ] **Step 7.1 : Vérifier le .gitignore actuel**

```bash
cat /Users/nassime/GitHub/Nexa-lang/.gitignore 2>/dev/null | head -30
```

- [ ] **Step 7.2 : Ajouter les entrées `.nexa/nex_out/` et `.nexa/compile/`**

Ajouter à la fin du `.gitignore` :

```
# Nexa build intermediates
.nexa/nex_out/
.nexa/compile/logs/
```

- [ ] **Step 7.3 : Commit**

```bash
git add .gitignore
git commit -m "chore: gitignore .nexa/nex_out and .nexa/compile/logs"
```

---

## Task 8 — Lancer tous les tests et vérifier la rétro-compatibilité

- [ ] **Step 8.1 : Lancer la suite de tests complète**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: tous les tests passent (au moins 273 — plus les nouveaux ajoutés dans ce plan).

- [ ] **Step 8.2 : Vérifier la rétro-compat web**

Créer un projet web minimal et compiler :

```bash
cd /tmp
nexa init retro-test
cd retro-test
nexa build
ls dist/core/browser/
```

Expected: `dist/core/browser/index.html` et `dist/core/browser/app.js` présents.

- [ ] **Step 8.3 : Vérifier nexa module add --type backend**

```bash
cd /tmp/retro-test
nexa module add myapi --type backend
cat modules/myapi/module.json
```

Expected:
```json
{
  "name": "myapi",
  "main": "app.nx",
  "dependencies": {},
  "type": "backend"
}
```

- [ ] **Step 8.4 : Commit final**

```bash
git add -A
git commit -m "feat(phase1): multi-target build — backend/cli Rust native, web rétro-compat"
```
