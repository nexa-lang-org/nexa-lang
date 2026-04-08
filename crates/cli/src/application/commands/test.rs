//! `nexa test` — compile and run `.nx` test files.
//!
//! Test files contain one or more `test "description" { stmts }` blocks.
//! Each block is compiled to a self-contained async JS function, wrapped in a
//! try/catch, and executed via `node`.  Results are printed to stdout.
//!
//! The built-in `assert(cond)` function throws on failure; any uncaught
//! exception in a test body is caught and reported as a failure.
//!
//! Exit code: 0 if all tests pass, 1 if any fail or `node` is unavailable.

use crate::infrastructure::ui;
use nexa_compiler::{
    application::services::{
        lexer::Lexer,
        parser::Parser,
    },
    domain::ast::{Declaration, Stmt, Expr},
};
use std::path::{Path, PathBuf};

// ── Public entry point ────────────────────────────────────────────────────────

pub fn test(project: Option<PathBuf>, filter: Option<String>) {
    // 1. Find test files
    let root = project.unwrap_or_else(|| PathBuf::from("."));
    let test_files = collect_test_files(&root);
    if test_files.is_empty() {
        println!("  No test files found (*.test.nx or files with test blocks).");
        return;
    }

    // 2. Check that `node` is available
    if std::process::Command::new("node").arg("--version").output().is_err() {
        ui::die("nexa test requires Node.js — install it from https://nodejs.org");
    }

    // 3. Compile + run each test file
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;

    for path in &test_files {
        let (p, f) = run_test_file(path, filter.as_deref());
        total += p + f;
        passed += p;
        failed += f;
    }

    // 4. Summary
    println!();
    if failed == 0 {
        println!("  All {total} test(s) passed.");
        std::process::exit(0);
    } else {
        eprintln!("  {passed}/{total} passed — {failed} failed.");
        std::process::exit(1);
    }
}

// ── Test file discovery ───────────────────────────────────────────────────────

fn collect_test_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_dir(root, &mut files);
    files
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories and target/
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" {
                continue;
            }
            walk_dir(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("nx") {
            // Include if it's a *.test.nx file OR contains test blocks
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".test.nx") || file_has_test_blocks(&path) {
                out.push(path);
            }
        }
    }
}

fn file_has_test_blocks(path: &Path) -> bool {
    let Ok(src) = std::fs::read_to_string(path) else { return false };
    let Ok(tokens) = Lexer::new(&src).tokenize() else { return false };
    let Ok(program) = Parser::new(tokens).parse_lib() else { return false };
    program.declarations.iter().any(|d| matches!(d, Declaration::Test(_)))
}

// ── Compile + run a single test file ─────────────────────────────────────────

fn run_test_file(path: &Path, filter: Option<&str>) -> (usize, usize) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  error reading {}: {e}", path.display());
            return (0, 1);
        }
    };

    let tokens = match Lexer::new(&src).tokenize() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  lex error in {}: {e}", path.display());
            return (0, 1);
        }
    };

    let program = match Parser::new(tokens).parse_lib() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  parse error in {}: {e}", path.display());
            return (0, 1);
        }
    };

    // Collect test blocks, applying filter
    let tests: Vec<_> = program
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::Test(t) => {
                if filter.map(|f| t.name.contains(f)).unwrap_or(true) {
                    Some(t)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    if tests.is_empty() {
        return (0, 0);
    }

    println!("  {}", path.display());

    // Build a Node.js test runner script
    let js = build_test_runner(&tests);

    // Write to a temp file and execute
    let tmp = std::env::temp_dir().join("_nexa_test_runner.mjs");
    if std::fs::write(&tmp, &js).is_err() {
        eprintln!("  failed to write temp test file");
        return (0, tests.len());
    }

    let output = std::process::Command::new("node")
        .arg(&tmp)
        .output()
        .expect("failed to run node");

    // Print stdout/stderr from the runner
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stdout.lines() {
        println!("    {line}");
    }
    for line in stderr.lines() {
        eprintln!("    {line}");
    }

    // Parse results: the runner prints "PASS:N FAIL:M" on the last line
    let last = stdout.lines().last().unwrap_or("");
    if let Some(counts) = parse_result_line(last) {
        counts
    } else {
        // If node exited non-zero treat all as failed
        (0, tests.len())
    }
}

fn parse_result_line(line: &str) -> Option<(usize, usize)> {
    // Format: "PASS:N FAIL:M"
    let mut pass = 0usize;
    let mut fail = 0usize;
    for token in line.split_whitespace() {
        if let Some(n) = token.strip_prefix("PASS:") {
            pass = n.parse().ok()?;
        } else if let Some(n) = token.strip_prefix("FAIL:") {
            fail = n.parse().ok()?;
        }
    }
    Some((pass, fail))
}

// ── JS test runner code generation ───────────────────────────────────────────

fn build_test_runner(
    tests: &[&nexa_compiler::domain::ast::TestDecl],
) -> String {
    let mut js = String::from(TEST_RUNTIME);
    js.push_str("const _tests = [];\n");

    for test in tests {
        let name_escaped = test.name.replace('\\', "\\\\").replace('"', "\\\"");
        js.push_str(&format!("_tests.push({{ name: \"{name_escaped}\", fn: async () => {{\n"));
        for stmt in &test.body {
            emit_stmt_js(stmt, &mut js, 1);
        }
        js.push_str("}}));\n");
    }

    js.push_str(TEST_RUNNER_BOOT);
    js
}

fn emit_stmt_js(stmt: &Stmt, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    match stmt {
        Stmt::Let { name, init, .. } => {
            out.push_str(&format!("{pad}let {name} = {};\n", emit_expr_js(init)));
        }
        Stmt::Assign { object, field, value } => {
            let target = match object {
                Expr::This => format!("this.{field}"),
                Expr::Ident(n) if n == field => n.clone(),
                other => format!("{}.{field}", emit_expr_js(other)),
            };
            out.push_str(&format!("{pad}{target} = {};\n", emit_expr_js(value)));
        }
        Stmt::Return { expr: Some(e), .. } => {
            out.push_str(&format!("{pad}return {};\n", emit_expr_js(e)));
        }
        Stmt::Return { expr: None, .. } => {
            out.push_str(&format!("{pad}return;\n"));
        }
        Stmt::Expr(e) => {
            out.push_str(&format!("{pad}{};\n", emit_expr_js(e)));
        }
        Stmt::If { cond, then_body, else_body } => {
            out.push_str(&format!("{pad}if ({}) {{\n", emit_expr_js(cond)));
            for s in then_body { emit_stmt_js(s, out, indent + 1); }
            if let Some(eb) = else_body {
                out.push_str(&format!("{pad}}} else {{\n"));
                for s in eb { emit_stmt_js(s, out, indent + 1); }
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::While { cond, body } => {
            out.push_str(&format!("{pad}while ({}) {{\n", emit_expr_js(cond)));
            for s in body { emit_stmt_js(s, out, indent + 1); }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::For { var, iter, body } => {
            out.push_str(&format!("{pad}for (const {var} of {}) {{\n", emit_expr_js(iter)));
            for s in body { emit_stmt_js(s, out, indent + 1); }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Break => out.push_str(&format!("{pad}break;\n")),
        Stmt::Continue => out.push_str(&format!("{pad}continue;\n")),
        Stmt::Match { expr, arms } => {
            out.push_str(&format!("{pad}const _m = {};\n", emit_expr_js(expr)));
            let mut first = true;
            for arm in arms {
                let cond_js = match &arm.pattern {
                    nexa_compiler::domain::ast::Pattern::Wildcard => None,
                    nexa_compiler::domain::ast::Pattern::Name(n) if n == "_" => None,
                    nexa_compiler::domain::ast::Pattern::Name(n) => {
                        Some(format!("_m._tag === \"{n}\""))
                    }
                    nexa_compiler::domain::ast::Pattern::QualifiedVariant { variant, .. } => {
                        Some(format!("_m._tag === \"{variant}\""))
                    }
                    nexa_compiler::domain::ast::Pattern::LitBool(b) => {
                        Some(format!("_m === {b}"))
                    }
                    nexa_compiler::domain::ast::Pattern::LitInt(n) => {
                        Some(format!("_m === {n}"))
                    }
                    nexa_compiler::domain::ast::Pattern::LitStr(s) => {
                        Some(format!("_m === \"{s}\""))
                    }
                };
                match (&cond_js, first) {
                    (Some(cond), true) => out.push_str(&format!("{pad}if ({cond}) {{\n")),
                    (Some(cond), false) => out.push_str(&format!("{pad}}} else if ({cond}) {{\n")),
                    (None, true) => out.push_str(&format!("{pad}{{\n")),
                    (None, false) => out.push_str(&format!("{pad}}} else {{\n")),
                }
                for s in &arm.body { emit_stmt_js(s, out, indent + 1); }
                first = false;
            }
            if !arms.is_empty() {
                out.push_str(&format!("{pad}}}\n"));
            }
        }
    }
}

fn emit_expr_js(expr: &Expr) -> String {
    match expr {
        Expr::IntLit(n) => n.to_string(),
        Expr::StringLit(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Ident(n) => n.clone(),
        Expr::This => "this".into(),
        Expr::FieldAccess(obj, field) => format!("{}.{field}", emit_expr_js(obj)),
        Expr::MethodCall { receiver, method, args } => {
            let args_js: Vec<_> = args.iter().map(emit_expr_js).collect();
            format!("{}.{method}({})", emit_expr_js(receiver), args_js.join(", "))
        }
        Expr::Call { callee, args } => {
            let args_js: Vec<_> = args.iter().map(emit_expr_js).collect();
            format!("{callee}({})", args_js.join(", "))
        }
        Expr::Binary { op, left, right } => {
            format!(
                "({} {} {})",
                emit_expr_js(left),
                op.as_js(),
                emit_expr_js(right)
            )
        }
        Expr::Unary { op, expr } => {
            let op_str = match op {
                nexa_compiler::domain::ast::UnOp::Not => "!",
                nexa_compiler::domain::ast::UnOp::Neg => "-",
            };
            format!("({op_str}{})", emit_expr_js(expr))
        }
        Expr::ListLiteral(items) => {
            let parts: Vec<_> = items.iter().map(emit_expr_js).collect();
            format!("[{}]", parts.join(", "))
        }
        Expr::Await(inner) => format!("(await {})", emit_expr_js(inner)),
        Expr::Block { tag, children } => {
            let kids: Vec<_> = children.iter().map(emit_expr_js).collect();
            format!("{tag}([{}])", kids.join(", "))
        }
        Expr::Lambda { params, body } => {
            let ps: Vec<_> = params.iter().map(|p| p.name.as_str()).collect();
            format!("({}) => {}", ps.join(", "), emit_expr_js(body))
        }
        Expr::LazyImport(path) => format!("import(\"{path}\")"),
    }
}

// ── Embedded test runtime ─────────────────────────────────────────────────────

const TEST_RUNTIME: &str = r#"
// Nexa test runtime
function assert(cond, msg) {
  if (!cond) throw new Error(msg || "Assertion failed");
}
function assertEqual(a, b, msg) {
  if (a !== b) throw new Error(msg || `Expected ${JSON.stringify(a)} to equal ${JSON.stringify(b)}`);
}
function assertNotEqual(a, b, msg) {
  if (a === b) throw new Error(msg || `Expected values to differ, both were ${JSON.stringify(a)}`);
}

"#;

const TEST_RUNNER_BOOT: &str = r#"
// Test runner
let _pass = 0, _fail = 0;
(async () => {
  for (const t of _tests) {
    try {
      await t.fn();
      _pass++;
      process.stdout.write("  \u2713 " + t.name + "\n");
    } catch (e) {
      _fail++;
      process.stdout.write("  \u2717 " + t.name + ": " + e.message + "\n");
    }
  }
  process.stdout.write("PASS:" + _pass + " FAIL:" + _fail + "\n");
  process.exit(_fail > 0 ? 1 : 0);
})();
"#;
