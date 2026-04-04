//! WASM backend — lowers an `IrModule` to WebAssembly Text format (WAT).
//!
//! # Usage
//! ```text
//! let wat = WasmCodegen::new().generate_wat(&ir_module)?;
//! std::fs::write("app.wat", &wat).unwrap();
//! // Assemble with: wat2wasm app.wat -o app.wasm
//! ```
//!
//! # Type mapping
//! | Nexa IR type     | WAT type      | Memory width |
//! |------------------|---------------|--------------|
//! | `Int`            | `i64`         | 8 bytes      |
//! | `Bool`           | `i32`         | 4 bytes      |
//! | `String`         | `i32` (ptr)   | 4 bytes      |
//! | `Named`/`List`   | `i32` (ptr)   | 4 bytes      |
//! | `Void`           | —             | —            |
//! | `Unknown`        | `i32`         | 4 bytes      |
//!
//! # Memory layout
//! ```text
//! [0 .. string_pool_end)  null-terminated string literals (data section)
//! [heap_start .. ∞)       bump-allocated objects
//! ```
//!
//! # Required JS host imports
//! ```javascript
//! const env = {
//!   dom_create_element:   (tagPtr)        => element,
//!   dom_set_text_content: (el, strPtr)    => void,
//!   dom_append_child:     (parent, child) => void,
//!   dom_query_selector:   (selectorPtr)   => element,
//!   console_log_i64:      (n)             => void,
//!   console_log_str:      (strPtr)        => void,
//! };
//! WebAssembly.instantiate(wasmBytes, { env }).then(({ instance }) => {
//!   instance.exports._nexa_start();
//! });
//! ```

use crate::domain::ir::*;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

// ── Error ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum WasmCodegenError {
    #[error("WASM codegen: {0}")]
    Error(String),
}

// ── Value types ────────────────────────────────────────────────────────────────

/// WASM register-level value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValTy {
    I32, // bool, pointer, string offset
    I64, // Int
}

impl ValTy {
    fn as_str(self) -> &'static str {
        match self {
            ValTy::I32 => "i32",
            ValTy::I64 => "i64",
        }
    }
    fn byte_size(self) -> u32 {
        match self {
            ValTy::I32 => 4,
            ValTy::I64 => 8,
        }
    }
    fn store(self, offset: u32) -> String {
        if offset == 0 {
            format!("{}.store", self.as_str())
        } else {
            format!("{}.store offset={}", self.as_str(), offset)
        }
    }
    fn load(self, offset: u32) -> String {
        if offset == 0 {
            format!("{}.load", self.as_str())
        } else {
            format!("{}.load offset={}", self.as_str(), offset)
        }
    }
}

fn ir_to_val(ty: &IrType) -> ValTy {
    match ty {
        IrType::Int => ValTy::I64,
        IrType::Bool
        | IrType::String
        | IrType::Void
        | IrType::Named(_)
        | IrType::List(_)
        | IrType::Fn(..)
        | IrType::Unknown => ValTy::I32,
    }
}

// ── Struct layout ──────────────────────────────────────────────────────────────

struct FieldSlot {
    offset: u32,
    vt: ValTy,
}

struct StructLayout {
    slots: HashMap<String, FieldSlot>,
    total_size: u32,
}

impl StructLayout {
    fn compute(cls: &IrClass) -> Self {
        let mut offset = 0u32;
        let mut slots = HashMap::new();
        for f in &cls.fields {
            let vt = ir_to_val(&f.ty);
            offset = align_up(offset, vt.byte_size());
            slots.insert(f.name.clone(), FieldSlot { offset, vt });
            offset += vt.byte_size();
        }
        StructLayout { slots, total_size: offset.max(4) }
    }

    fn get(&self, name: &str) -> Option<(u32, ValTy)> {
        self.slots.get(name).map(|s| (s.offset, s.vt))
    }
}

fn align_up(n: u32, align: u32) -> u32 {
    if align == 0 { n } else { (n + align - 1) & !(align - 1) }
}

// ── String pool ────────────────────────────────────────────────────────────────

struct StringPool {
    data: Vec<u8>,
    index: HashMap<String, u32>,
}

impl StringPool {
    fn new() -> Self {
        StringPool { data: Vec::new(), index: HashMap::new() }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.index.get(s) {
            return off;
        }
        let off = self.data.len() as u32;
        self.index.insert(s.to_string(), off);
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0); // null terminator
        off
    }

    fn heap_start(&self) -> u32 {
        align_up(self.data.len() as u32, 8).max(8)
    }

    /// WAT-escaped data string (for the `(data ...)` instruction).
    fn escaped(&self) -> String {
        let mut out = String::with_capacity(self.data.len() * 2);
        for &b in &self.data {
            match b {
                b'"' => out.push_str("\\22"),
                b'\\' => out.push_str("\\5c"),
                0 => out.push_str("\\00"),
                c if c.is_ascii_graphic() || c == b' ' => out.push(c as char),
                c => out.push_str(&format!("\\{:02x}", c)),
            }
        }
        out
    }
}

// ── Local collection ───────────────────────────────────────────────────────────

/// Collect all `IrStmt::Let` bindings (function-scoped in WASM).
fn collect_let_locals(stmts: &[IrStmt], out: &mut Vec<(String, ValTy)>) {
    for stmt in stmts {
        match stmt {
            IrStmt::Let { name, ty, .. } => {
                if !out.iter().any(|(n, _)| n == name) {
                    out.push((name.clone(), ir_to_val(ty)));
                }
            }
            IrStmt::If { then_body, else_body, .. } => {
                collect_let_locals(then_body, out);
                if let Some(eb) = else_body {
                    collect_let_locals(eb, out);
                }
            }
            IrStmt::While { body, .. } | IrStmt::For { body, .. } => {
                collect_let_locals(body, out);
            }
            _ => {}
        }
    }
}

/// Count `IrExpr::Node` occurrences (DFS) — each needs a pre-declared local.
fn count_nodes_in_stmts(stmts: &[IrStmt]) -> usize {
    stmts.iter().map(count_nodes_in_stmt).sum()
}

fn count_nodes_in_stmt(stmt: &IrStmt) -> usize {
    match stmt {
        IrStmt::Let { init, .. } | IrStmt::Discard(init) => count_nodes_in_expr(init),
        IrStmt::Assign { target, value } => {
            count_nodes_in_expr(target) + count_nodes_in_expr(value)
        }
        IrStmt::Return(Some(e)) => count_nodes_in_expr(e),
        IrStmt::If { cond, then_body, else_body } => {
            count_nodes_in_expr(cond)
                + count_nodes_in_stmts(then_body)
                + else_body.as_deref().map_or(0, count_nodes_in_stmts)
        }
        IrStmt::While { cond, body } => {
            count_nodes_in_expr(cond) + count_nodes_in_stmts(body)
        }
        IrStmt::For { iter, body, .. } => {
            count_nodes_in_expr(iter) + count_nodes_in_stmts(body)
        }
        _ => 0,
    }
}

fn count_nodes_in_expr(expr: &IrExpr) -> usize {
    match expr {
        IrExpr::Node { children, .. } => {
            1 + children.iter().map(count_nodes_in_expr).sum::<usize>()
        }
        IrExpr::Bin { lhs, rhs, .. } => {
            count_nodes_in_expr(lhs) + count_nodes_in_expr(rhs)
        }
        IrExpr::Unary { operand, .. } => count_nodes_in_expr(operand),
        IrExpr::Field { receiver, .. } => count_nodes_in_expr(receiver),
        IrExpr::Call { receiver, args, .. } => {
            count_nodes_in_expr(receiver) + args.iter().map(count_nodes_in_expr).sum::<usize>()
        }
        IrExpr::Invoke { args, .. } => args.iter().map(count_nodes_in_expr).sum(),
        IrExpr::Closure { body, .. } => count_nodes_in_expr(body),
        _ => 0,
    }
}

// ── String scanning ────────────────────────────────────────────────────────────

fn scan_strings_stmts(stmts: &[IrStmt], pool: &mut StringPool) {
    for stmt in stmts {
        match stmt {
            IrStmt::Let { init, .. } | IrStmt::Discard(init) => {
                scan_strings_expr(init, pool);
            }
            IrStmt::Assign { target, value } => {
                scan_strings_expr(target, pool);
                scan_strings_expr(value, pool);
            }
            IrStmt::Return(Some(e)) => scan_strings_expr(e, pool),
            IrStmt::If { cond, then_body, else_body } => {
                scan_strings_expr(cond, pool);
                scan_strings_stmts(then_body, pool);
                if let Some(eb) = else_body {
                    scan_strings_stmts(eb, pool);
                }
            }
            IrStmt::While { cond, body } => {
                scan_strings_expr(cond, pool);
                scan_strings_stmts(body, pool);
            }
            IrStmt::For { iter, body, .. } => {
                scan_strings_expr(iter, pool);
                scan_strings_stmts(body, pool);
            }
            _ => {}
        }
    }
}

fn scan_strings_expr(expr: &IrExpr, pool: &mut StringPool) {
    match expr {
        IrExpr::Str(s) => {
            pool.intern(s);
        }
        IrExpr::Node { tag, children } => {
            pool.intern(tag);
            for c in children {
                scan_strings_expr(c, pool);
            }
        }
        IrExpr::Bin { lhs, rhs, .. } => {
            scan_strings_expr(lhs, pool);
            scan_strings_expr(rhs, pool);
        }
        IrExpr::Unary { operand, .. } => scan_strings_expr(operand, pool),
        IrExpr::Field { receiver, .. } => scan_strings_expr(receiver, pool),
        IrExpr::Call { receiver, args, .. } => {
            scan_strings_expr(receiver, pool);
            for a in args {
                scan_strings_expr(a, pool);
            }
        }
        IrExpr::Invoke { args, .. } => {
            for a in args {
                scan_strings_expr(a, pool);
            }
        }
        IrExpr::Closure { body, .. } => scan_strings_expr(body, pool),
        _ => {}
    }
}

// ── WAT generator ─────────────────────────────────────────────────────────────

struct WatGen {
    out: String,
    indent: usize,
    pool: StringPool,
    layouts: HashMap<String, StructLayout>,
    /// (class_name, method_name) pairs where the method returns Void.
    void_methods: HashSet<(String, String)>,
    loop_idx: usize,
    node_idx: usize,
    current_class: String,
}

impl WatGen {
    fn new() -> Self {
        WatGen {
            out: String::new(),
            indent: 0,
            pool: StringPool::new(),
            layouts: HashMap::new(),
            void_methods: HashSet::new(),
            loop_idx: 0,
            node_idx: 0,
            current_class: String::new(),
        }
    }

    // ── Output helpers ────────────────────────────────────────────────────────

    fn ln(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn blank(&mut self) {
        self.out.push('\n');
    }

    fn comment(&mut self, s: &str) {
        let indent = "  ".repeat(self.indent);
        self.out.push_str(&format!("{};; {}\n", indent, s));
    }

    fn section(&mut self, title: &str) {
        self.blank();
        let bar = "─".repeat(60usize.saturating_sub(title.len() + 2));
        self.comment(&format!("─── {} {}", title, bar));
    }

    // ── Top-level entry ───────────────────────────────────────────────────────

    fn compile(&mut self, module: &IrModule) -> Result<(), WasmCodegenError> {
        // Phase 1 — collect strings and build layouts.
        self.pool.intern("#app");
        for cls in &module.classes {
            scan_strings_stmts(&cls.constructor_body, &mut self.pool);
            for m in &cls.methods {
                scan_strings_stmts(&m.body, &mut self.pool);
                if m.return_ty == IrType::Void {
                    self.void_methods.insert((cls.name.clone(), m.name.clone()));
                }
            }
            self.layouts.insert(cls.name.clone(), StructLayout::compute(cls));
        }

        let heap_start = self.pool.heap_start();

        // Phase 2 — emit WAT.
        self.out.push_str(";; Generated by nexa-compiler — WASM target\n");
        self.out.push_str(";; Assemble: wat2wasm app.wat -o app.wasm\n");
        self.ln("(module");
        self.indent += 1;

        self.emit_memory(heap_start)?;
        self.emit_imports();
        self.emit_alloc(heap_start);

        for cls in &module.classes {
            self.compile_class(cls)?;
        }

        self.emit_start(module)?;

        self.indent -= 1;
        self.ln(")");
        Ok(())
    }

    // ── Memory + data section ─────────────────────────────────────────────────

    fn emit_memory(&mut self, heap_start: u32) -> Result<(), WasmCodegenError> {
        self.section("Memory (2 pages = 128 KiB)");
        self.ln("(memory (export \"memory\") 2)");

        if !self.pool.data.is_empty() {
            self.blank();
            self.comment("String pool — null-terminated");
            self.ln(&format!("(data (i32.const 0) \"{}\")", self.pool.escaped()));
            let mut pairs: Vec<(u32, String)> =
                self.pool.index.iter().map(|(s, &o)| (o, s.clone())).collect();
            pairs.sort_by_key(|(o, _)| *o);
            for (off, s) in pairs {
                let disp = if s.len() > 40 { s[..40].to_string() } else { s.clone() };
                self.comment(&format!("  @{}: \"{}\"", off, disp));
            }
        }

        self.blank();
        self.comment(&format!("Heap starts at offset {}", heap_start));
        Ok(())
    }

    // ── JS imports ────────────────────────────────────────────────────────────

    fn emit_imports(&mut self) {
        self.section("JS host imports (DOM / I/O)");
        let imports: &[(&str, &str)] = &[
            ("dom_create_element", "(param i32) (result i32)"),
            ("dom_set_text_content", "(param i32 i32)"),
            ("dom_append_child", "(param i32 i32)"),
            ("dom_query_selector", "(param i32) (result i32)"),
            ("console_log_i64", "(param i64)"),
            ("console_log_str", "(param i32)"),
        ];
        for (name, sig) in imports {
            self.ln(&format!(
                "(import \"env\" \"{name}\" (func $env_{name} {sig}))"
            ));
        }
    }

    // ── Bump allocator ────────────────────────────────────────────────────────

    fn emit_alloc(&mut self, heap_start: u32) {
        self.section("Bump allocator");
        self.ln(&format!(
            "(global $__heap_ptr (mut i32) (i32.const {}))",
            heap_start
        ));
        self.blank();
        self.ln("(func $__alloc (param $size i32) (result i32)");
        self.indent += 1;
        self.ln("(local $ptr i32)");
        self.ln("(local.set $ptr (global.get $__heap_ptr))");
        self.ln("(global.set $__heap_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__heap_ptr) (local.get $size)))");
        self.indent -= 1;
        self.ln("(local.get $ptr))");
        self.indent -= 1;
    }

    // ── Class compilation ─────────────────────────────────────────────────────

    fn compile_class(&mut self, cls: &IrClass) -> Result<(), WasmCodegenError> {
        self.section(&format!("class {} ({:?})", cls.name, cls.kind));

        if let Some(layout) = self.layouts.get(&cls.name) {
            let mut slots: Vec<(String, u32, ValTy)> = layout
                .slots
                .iter()
                .map(|(n, s)| (n.clone(), s.offset, s.vt))
                .collect();
            let total_size = layout.total_size;
            slots.sort_by_key(|(_, off, _)| *off);
            let desc: Vec<String> = slots
                .iter()
                .map(|(n, off, vt)| format!("{}: {} @{}", n, vt.as_str(), off))
                .collect();
            if !desc.is_empty() {
                self.comment(&format!("struct {{ {} }}", desc.join(", ")));
            }
            self.comment(&format!("total = {} bytes", total_size));
        }
        self.blank();

        self.emit_constructor(cls)?;

        self.current_class = cls.name.clone();
        for method in &cls.methods {
            self.blank();
            self.emit_method(&cls.name.clone(), method)?;
        }
        Ok(())
    }

    fn emit_constructor(&mut self, cls: &IrClass) -> Result<(), WasmCodegenError> {
        let size = self.layouts.get(&cls.name).map(|l| l.total_size).unwrap_or(4);
        let params: String = cls
            .constructor_params
            .iter()
            .map(|p| format!("(param ${} {})", p.name, ir_to_val(&p.ty).as_str()))
            .collect::<Vec<_>>()
            .join(" ");

        let header = if params.is_empty() {
            format!("(func ${}_new (result i32)", cls.name)
        } else {
            format!("(func ${}_new {} (result i32)", cls.name, params)
        };

        self.ln(&header);
        self.indent += 1;
        self.ln("(local $self i32)");
        self.ln(&format!("(local.set $self (call $__alloc (i32.const {})))", size));

        // Store each param into its corresponding field.
        for p in &cls.constructor_params {
            let vt = ir_to_val(&p.ty);
            if let Some(layout) = self.layouts.get(&cls.name) {
                if let Some((offset, _)) = layout.get(&p.name) {
                    self.ln(&format!(
                        "({} (local.get $self) (local.get ${}))",
                        vt.store(offset),
                        p.name
                    ));
                }
            }
        }

        // Emit explicit constructor body if present.
        if !cls.constructor_body.is_empty() {
            self.current_class = cls.name.clone();
            let mut let_locals: Vec<(String, ValTy)> = vec![("self".to_string(), ValTy::I32)];
            collect_let_locals(&cls.constructor_body, &mut let_locals);
            let local_map: HashMap<String, ValTy> = let_locals.into_iter().collect();
            self.emit_stmts(&cls.constructor_body.clone(), &local_map)?;
        }

        self.ln("(local.get $self))");
        self.indent -= 1;
        Ok(())
    }

    fn emit_method(&mut self, class_name: &str, method: &IrMethod) -> Result<(), WasmCodegenError> {
        // Param signature: self (i32 pointer) + declared params.
        let mut param_parts = vec!["(param $self i32)".to_string()];
        param_parts.extend(method.params.iter().map(|p| {
            format!("(param ${} {})", p.name, ir_to_val(&p.ty).as_str())
        }));
        let params_str = param_parts.join(" ");

        let result_str = match &method.return_ty {
            IrType::Void => String::new(),
            ty => format!(" (result {})", ir_to_val(ty).as_str()),
        };

        self.comment(&format!(
            "{}.{}(self{}) -> {}",
            class_name,
            method.name,
            if method.params.is_empty() { String::new() } else {
                format!(", {}", method.params.iter()
                    .map(|p| format!("{}: {}", p.name, ir_to_val(&p.ty).as_str()))
                    .collect::<Vec<_>>().join(", "))
            },
            match &method.return_ty { IrType::Void => "void", ty => ir_to_val(ty).as_str() }
        ));

        self.ln(&format!(
            "(func ${}_{} {}{}",
            class_name, method.name, params_str, result_str
        ));
        self.indent += 1;

        // Build local type map for expression emission.
        let mut locals: HashMap<String, ValTy> = HashMap::new();
        locals.insert("self".to_string(), ValTy::I32);
        for p in &method.params {
            locals.insert(p.name.clone(), ir_to_val(&p.ty));
        }

        // Declare let-bound locals.
        let mut let_locals: Vec<(String, ValTy)> = Vec::new();
        collect_let_locals(&method.body, &mut let_locals);
        for (name, vt) in &let_locals {
            self.ln(&format!("(local ${} {})", name, vt.as_str()));
            locals.insert(name.clone(), *vt);
        }

        // Declare node locals (one i32 per IrExpr::Node in DFS order).
        let node_count = count_nodes_in_stmts(&method.body);
        for i in 0..node_count {
            self.ln(&format!("(local $__node_{} i32)", i));
            locals.insert(format!("__node_{}", i), ValTy::I32);
        }

        // Reset node counter before emission so indices match declarations.
        self.node_idx = 0;
        self.emit_stmts(&method.body.clone(), &locals)?;

        self.ln(")");
        self.indent -= 1;
        Ok(())
    }

    // ── Statement emission ────────────────────────────────────────────────────

    fn emit_stmts(
        &mut self,
        stmts: &[IrStmt],
        locals: &HashMap<String, ValTy>,
    ) -> Result<(), WasmCodegenError> {
        for stmt in stmts {
            self.emit_stmt(stmt, locals)?;
        }
        Ok(())
    }

    fn emit_stmt(
        &mut self,
        stmt: &IrStmt,
        locals: &HashMap<String, ValTy>,
    ) -> Result<(), WasmCodegenError> {
        match stmt {
            IrStmt::Let { name, init, .. } => {
                self.emit_expr(init, locals)?;
                self.ln(&format!("local.set ${}", name));
            }
            IrStmt::Assign { target, value } => {
                // Stack: addr, value → store.
                match target {
                    IrExpr::Field { receiver, name } => {
                        let cls = self.current_class.clone();
                        let (offset, vt) = self
                            .layouts
                            .get(&cls)
                            .and_then(|l| l.get(name))
                            .unwrap_or((0, ValTy::I32));
                        self.emit_expr(receiver, locals)?;
                        self.emit_expr(value, locals)?;
                        self.ln(&vt.store(offset));
                    }
                    _ => {
                        self.emit_expr(value, locals)?;
                        self.ln("drop  ;; assign to non-field target");
                    }
                }
            }
            IrStmt::Return(None) => {
                self.ln("return");
            }
            IrStmt::Return(Some(e)) => {
                self.emit_expr(e, locals)?;
                self.ln("return");
            }
            IrStmt::Discard(e) => {
                let pushes_value = self.emit_expr(e, locals)?;
                if pushes_value {
                    self.ln("drop");
                }
            }
            IrStmt::If { cond, then_body, else_body } => {
                self.emit_expr(cond, locals)?;
                // Ensure i32 on stack (WASM if needs i32).
                // Comparisons and logical ops already produce i32; Int values need a cast.
                let cond_ty = self.infer_valtype(cond, locals);
                if cond_ty == ValTy::I64 {
                    self.ln("i64.const 0");
                    self.ln("i64.ne  ;; coerce i64 to bool (i32)");
                }
                if let Some(eb) = else_body {
                    self.ln("if");
                    self.indent += 1;
                    self.emit_stmts(then_body, locals)?;
                    self.indent -= 1;
                    self.ln("else");
                    self.indent += 1;
                    self.emit_stmts(eb, locals)?;
                    self.indent -= 1;
                    self.ln("end");
                } else {
                    self.ln("if");
                    self.indent += 1;
                    self.emit_stmts(then_body, locals)?;
                    self.indent -= 1;
                    self.ln("end");
                }
            }
            IrStmt::While { cond, body } => {
                let idx = self.loop_idx;
                self.loop_idx += 1;
                self.ln(&format!("block $brk_{}", idx));
                self.indent += 1;
                self.ln(&format!("loop $lp_{}", idx));
                self.indent += 1;
                self.emit_expr(cond, locals)?;
                let cond_ty = self.infer_valtype(cond, locals);
                if cond_ty == ValTy::I64 {
                    self.ln("i64.const 0");
                    self.ln("i64.ne");
                }
                self.ln("i32.eqz");
                self.ln(&format!("br_if $brk_{}", idx));
                self.emit_stmts(body, locals)?;
                self.ln(&format!("br $lp_{}", idx));
                self.indent -= 1;
                self.ln("end");
                self.indent -= 1;
                self.ln("end");
            }
            IrStmt::For { var, iter, body } => {
                // `for var in iter` — requires JS-side iterator; emit structural loop.
                let idx = self.loop_idx;
                self.loop_idx += 1;
                self.comment(&format!("for {} in iter — iterator needs JS interop", var));
                self.emit_expr(iter, locals)?;
                self.ln("drop  ;; discard iterator (JS-managed)");
                self.ln(&format!("block $brk_{}", idx));
                self.indent += 1;
                self.ln(&format!("loop $lp_{}", idx));
                self.indent += 1;
                self.comment("loop body");
                self.emit_stmts(body, locals)?;
                self.ln(&format!("br $lp_{}", idx));
                self.indent -= 1;
                self.ln("end");
                self.indent -= 1;
                self.ln("end");
            }
            IrStmt::Break => {
                let idx = self.loop_idx.saturating_sub(1);
                self.ln(&format!("br $brk_{}", idx));
            }
            IrStmt::Continue => {
                let idx = self.loop_idx.saturating_sub(1);
                self.ln(&format!("br $lp_{}", idx));
            }
        }
        Ok(())
    }

    // ── Expression emission ───────────────────────────────────────────────────
    //
    // Returns `true` if the expression leaves a value on the WASM stack.

    fn emit_expr(
        &mut self,
        expr: &IrExpr,
        locals: &HashMap<String, ValTy>,
    ) -> Result<bool, WasmCodegenError> {
        let pushed = match expr {
            IrExpr::Int(n) => {
                self.ln(&format!("i64.const {}", n));
                true
            }
            IrExpr::Bool(b) => {
                self.ln(&format!("i32.const {}", if *b { 1 } else { 0 }));
                true
            }
            IrExpr::Str(s) => {
                let off = self.pool.index.get(s.as_str()).copied().unwrap_or(0);
                self.ln(&format!("i32.const {}  ;; \"{}\"", off, s.escape_default()));
                true
            }
            IrExpr::Local(name) => {
                self.ln(&format!("local.get ${}", name));
                true
            }
            IrExpr::SelfRef => {
                self.ln("local.get $self");
                true
            }
            IrExpr::Field { receiver, name } => {
                self.emit_expr(receiver, locals)?;
                let cls = self.current_class.clone();
                let (offset, vt) = self
                    .layouts
                    .get(&cls)
                    .and_then(|l| l.get(name))
                    .unwrap_or((0, ValTy::I32));
                self.ln(&vt.load(offset));
                true
            }
            IrExpr::Bin { op, lhs, rhs } => {
                let lhs_ty = self.infer_valtype(lhs, locals);
                self.emit_expr(lhs, locals)?;
                self.emit_expr(rhs, locals)?;
                self.ln(binop_instr(op, lhs_ty));
                true
            }
            IrExpr::Unary { op, operand } => {
                match op {
                    IrUnOp::Not => {
                        self.emit_expr(operand, locals)?;
                        self.ln("i32.eqz");
                    }
                    IrUnOp::Neg => {
                        self.ln("i64.const 0");
                        self.emit_expr(operand, locals)?;
                        self.ln("i64.sub");
                    }
                }
                true
            }
            IrExpr::Call { receiver, method, args } => {
                // Push self (receiver) then each argument.
                self.emit_expr(receiver, locals)?;
                for arg in args {
                    self.emit_expr(arg, locals)?;
                }
                let cls = self.current_class.clone();
                self.ln(&format!("call ${}_{}", cls, method));
                // Return false (no stack value) if method is known-void.
                !self.void_methods.contains(&(cls, method.clone()))
            }
            IrExpr::Invoke { callee, args } => {
                // Constructor: push args, call $Callee_new → i32 pointer.
                for arg in args {
                    self.emit_expr(arg, locals)?;
                }
                self.ln(&format!("call ${}_new", callee));
                true // constructors always return i32
            }
            IrExpr::Node { tag, children } => {
                // Create element, optionally set text / append child nodes.
                let tag_off = self.pool.index.get(tag.as_str()).copied().unwrap_or(0);
                let node_local = format!("$__node_{}", self.node_idx);
                self.node_idx += 1;

                self.ln(&format!("i32.const {}  ;; tag \"{}\"", tag_off, tag));
                self.ln("call $env_dom_create_element");
                self.ln(&format!("local.set {}", node_local));

                for child in children {
                    if let IrExpr::Str(s) = child {
                        // String child → set text content.
                        let str_off =
                            self.pool.index.get(s.as_str()).copied().unwrap_or(0);
                        self.ln(&format!("local.get {}  ;; element", node_local));
                        self.ln(&format!("i32.const {}  ;; \"{}\"", str_off, s.escape_default()));
                        self.ln("call $env_dom_set_text_content");
                    } else {
                        // DOM child → append.
                        self.ln(&format!("local.get {}  ;; parent", node_local));
                        let child_pushes = self.emit_expr(child, locals)?;
                        if child_pushes {
                            self.ln("call $env_dom_append_child");
                        }
                    }
                }

                self.ln(&format!("local.get {}", node_local));
                true
            }
            IrExpr::Closure { .. } => {
                // Function-references proposal not assumed — emit a null ref placeholder.
                self.ln(
                    "i32.const 0  ;; closure (function-references not yet supported)",
                );
                true
            }
            // Await / list / dynamic-import are JS-target concepts; emit null in WASM.
            IrExpr::Await(inner) => {
                self.emit_expr(inner, locals)?;
                false // value stays on stack (passthrough)
            }
            IrExpr::List(_) => {
                self.ln("i32.const 0  ;; list (not yet supported in WASM backend)");
                true
            }
            IrExpr::DynamicImport(_) => {
                self.ln("i32.const 0  ;; dynamic import (not yet supported in WASM backend)");
                true
            }
        };
        Ok(pushed)
    }

    // ── Type inference (for binary op dispatch) ───────────────────────────────

    fn infer_valtype(&self, expr: &IrExpr, locals: &HashMap<String, ValTy>) -> ValTy {
        match expr {
            IrExpr::Int(_) => ValTy::I64,
            IrExpr::Bool(_) | IrExpr::Str(_) | IrExpr::SelfRef => ValTy::I32,
            IrExpr::Local(name) => locals.get(name).copied().unwrap_or(ValTy::I32),
            IrExpr::Field { name, .. } => self
                .layouts
                .get(&self.current_class)
                .and_then(|l| l.get(name))
                .map(|(_, vt)| vt)
                .unwrap_or(ValTy::I32),
            IrExpr::Bin { op: IrBinOp::Add | IrBinOp::Sub | IrBinOp::Mul | IrBinOp::Div | IrBinOp::Mod, lhs, .. } => {
                self.infer_valtype(lhs, locals)
            }
            IrExpr::Bin { .. } => ValTy::I32, // comparisons and logical → bool (i32)
            IrExpr::Unary { op, .. } => match op {
                IrUnOp::Not => ValTy::I32,
                IrUnOp::Neg => ValTy::I64,
            },
            _ => ValTy::I32,
        }
    }

    // ── Application entry point ───────────────────────────────────────────────

    fn emit_start(&mut self, module: &IrModule) -> Result<(), WasmCodegenError> {
        self.section("Application entry point");
        self.blank();
        self.ln("(func $_nexa_start (export \"_nexa_start\")");
        self.indent += 1;

        let app_sel_off = self.pool.index.get("#app").copied().unwrap_or(0);
        let main_win = module.routes.iter().find(|r| r.path == "/").map(|r| r.target.as_str());

        if let Some(win) = main_win {
            self.ln("(local $root_el i32)");
            self.ln("(local $window_inst i32)");
            self.ln("(local $page i32)");
            self.ln(&format!(
                "(local.set $root_el (call $env_dom_query_selector (i32.const {})))",
                app_sel_off
            ));
            self.ln(&format!("(local.set $window_inst (call ${}_new))", win));
            self.ln(&format!(
                "(local.set $page (call ${}_render (local.get $window_inst)))",
                win
            ));
            self.ln("(call $env_dom_append_child (local.get $root_el) (local.get $page))");
        } else {
            self.comment("No route \"/\" defined — nothing to render at startup");
        }

        if let Some(srv) = &module.server {
            self.comment(&format!(
                "Server port {} — handled by the JS runtime layer",
                srv.port
            ));
        }

        self.ln(")");
        self.indent -= 1;
        Ok(())
    }
}

// ── Binary operation instructions ─────────────────────────────────────────────

fn binop_instr(op: &IrBinOp, lhs_ty: ValTy) -> &'static str {
    match (op, lhs_ty) {
        (IrBinOp::Add, ValTy::I64) => "i64.add",
        (IrBinOp::Sub, ValTy::I64) => "i64.sub",
        (IrBinOp::Mul, ValTy::I64) => "i64.mul",
        (IrBinOp::Div, ValTy::I64) => "i64.div_s",
        (IrBinOp::Mod, ValTy::I64) => "i64.rem_s",
        (IrBinOp::Eq, ValTy::I64) => "i64.eq",
        (IrBinOp::Ne, ValTy::I64) => "i64.ne",
        (IrBinOp::Lt, ValTy::I64) => "i64.lt_s",
        (IrBinOp::Gt, ValTy::I64) => "i64.gt_s",
        (IrBinOp::Le, ValTy::I64) => "i64.le_s",
        (IrBinOp::Ge, ValTy::I64) => "i64.ge_s",
        (IrBinOp::Add, ValTy::I32) => "i32.add",
        (IrBinOp::Sub, ValTy::I32) => "i32.sub",
        (IrBinOp::Mul, ValTy::I32) => "i32.mul",
        (IrBinOp::Div, ValTy::I32) => "i32.div_s",
        (IrBinOp::Mod, ValTy::I32) => "i32.rem_s",
        (IrBinOp::Eq, ValTy::I32) => "i32.eq",
        (IrBinOp::Ne, ValTy::I32) => "i32.ne",
        (IrBinOp::Lt, ValTy::I32) => "i32.lt_s",
        (IrBinOp::Gt, ValTy::I32) => "i32.gt_s",
        (IrBinOp::Le, ValTy::I32) => "i32.le_s",
        (IrBinOp::Ge, ValTy::I32) => "i32.ge_s",
        (IrBinOp::And, _) => "i32.and",
        (IrBinOp::Or, _) => "i32.or",
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// WASM backend: lowers an [`IrModule`] to WebAssembly Text format (WAT).
///
/// Assemble the resulting WAT with `wat2wasm app.wat -o app.wasm`.
pub struct WasmCodegen;

impl WasmCodegen {
    pub fn new() -> Self {
        WasmCodegen
    }

    /// Generate WAT source for `ir`.
    pub fn generate_wat(&self, ir: &IrModule) -> Result<String, WasmCodegenError> {
        let mut gen = WatGen::new();
        gen.compile(ir)?;
        Ok(gen.out)
    }
}

impl Default for WasmCodegen {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn counter_ir() -> IrModule {
        IrModule {
            name: "Counter".into(),
            server: Some(IrServer { port: 3000 }),
            classes: vec![IrClass {
                name: "Counter".into(),
                kind: IrClassKind::Class,
                is_public: true,
                fields: vec![IrField {
                    name: "count".into(),
                    ty: IrType::Int,
                    is_public: false,
                }],
                constructor_params: vec![IrParam {
                    name: "count".into(),
                    ty: IrType::Int,
                }],
                constructor_body: vec![],
                methods: vec![
                    IrMethod {
                        name: "get_count".into(),
                        params: vec![],
                        return_ty: IrType::Int,
                        body: vec![IrStmt::Return(Some(IrExpr::Field {
                            receiver: Box::new(IrExpr::SelfRef),
                            name: "count".into(),
                        }))],
                        is_public: true,
            is_async: false,
                    },
                    IrMethod {
                        name: "increment".into(),
                        params: vec![],
                        return_ty: IrType::Void,
                        body: vec![IrStmt::Assign {
                            target: IrExpr::Field {
                                receiver: Box::new(IrExpr::SelfRef),
                                name: "count".into(),
                            },
                            value: IrExpr::Bin {
                                op: IrBinOp::Add,
                                lhs: Box::new(IrExpr::Field {
                                    receiver: Box::new(IrExpr::SelfRef),
                                    name: "count".into(),
                                }),
                                rhs: Box::new(IrExpr::Int(1)),
                            },
                        }],
                        is_public: true,
            is_async: false,
                    },
                ],
            }],
            routes: vec![],
        }
    }

    fn window_ir() -> IrModule {
        IrModule {
            name: "App".into(),
            server: Some(IrServer { port: 3000 }),
            classes: vec![IrClass {
                name: "HomePage".into(),
                kind: IrClassKind::Window,
                is_public: true,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![IrMethod {
                    name: "render".into(),
                    params: vec![],
                    return_ty: IrType::Named("Component".into()),
                    body: vec![IrStmt::Return(Some(IrExpr::Node {
                        tag: "Page".into(),
                        children: vec![IrExpr::Node {
                            tag: "Heading".into(),
                            children: vec![IrExpr::Str("Hello!".into())],
                        }],
                    }))],
                    is_public: true,
            is_async: false,
                }],
            }],
            routes: vec![IrRoute { path: "/".into(), target: "HomePage".into() }],
        }
    }

    fn wat(ir: &IrModule) -> String {
        WasmCodegen::new().generate_wat(ir).unwrap()
    }

    #[test]
    fn output_starts_and_ends_as_module() {
        let w = wat(&counter_ir());
        assert!(w.contains("(module"), "should open with (module");
        // Last non-empty line should close it.
        let last = w.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("");
        assert_eq!(last, ")", "should close module with )");
    }

    #[test]
    fn output_declares_exported_memory() {
        let w = wat(&counter_ir());
        assert!(w.contains("(memory"), "should declare memory");
        assert!(w.contains("(export \"memory\")"), "memory should be exported");
    }

    #[test]
    fn output_has_dom_imports() {
        let w = wat(&window_ir());
        assert!(w.contains("dom_create_element"), "missing dom_create_element import");
        assert!(w.contains("dom_append_child"), "missing dom_append_child import");
        assert!(w.contains("dom_query_selector"), "missing dom_query_selector import");
    }

    #[test]
    fn output_has_alloc() {
        let w = wat(&counter_ir());
        assert!(w.contains("$__alloc"), "__alloc function missing");
        assert!(w.contains("$__heap_ptr"), "__heap_ptr global missing");
    }

    #[test]
    fn class_generates_constructor() {
        let w = wat(&counter_ir());
        assert!(w.contains("$Counter_new"), "Counter_new missing");
    }

    #[test]
    fn class_generates_methods() {
        let w = wat(&counter_ir());
        assert!(w.contains("$Counter_get_count"), "Counter_get_count missing");
        assert!(w.contains("$Counter_increment"), "Counter_increment missing");
    }

    #[test]
    fn int_field_uses_i64() {
        let w = wat(&counter_ir());
        assert!(w.contains("i64.store"), "int field store should use i64.store");
        assert!(w.contains("i64.load"), "int field load should use i64.load");
    }

    #[test]
    fn int_arithmetic_uses_i64_add() {
        let w = wat(&counter_ir());
        assert!(w.contains("i64.add"), "int arithmetic should use i64.add");
    }

    #[test]
    fn window_generates_start_export() {
        let w = wat(&window_ir());
        assert!(w.contains("_nexa_start"), "_nexa_start missing");
        assert!(w.contains("export \"_nexa_start\""), "_nexa_start should be exported");
    }

    #[test]
    fn string_literals_go_into_data_section() {
        let w = wat(&window_ir());
        assert!(w.contains("(data (i32.const 0)"), "data section missing");
        assert!(w.contains("Hello!"), "string literal missing from data");
    }

    #[test]
    fn node_calls_dom_create_element() {
        let w = wat(&window_ir());
        assert!(w.contains("call $env_dom_create_element"), "dom_create_element not called");
    }

    #[test]
    fn str_child_calls_dom_set_text_content() {
        let w = wat(&window_ir());
        assert!(w.contains("call $env_dom_set_text_content"), "text content not set");
    }

    #[test]
    fn bool_literal_emits_i32_const() {
        let ir = IrModule {
            name: "Test".into(),
            server: None,
            classes: vec![IrClass {
                name: "T".into(),
                kind: IrClassKind::Class,
                is_public: false,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![IrMethod {
                    name: "ok".into(),
                    params: vec![],
                    return_ty: IrType::Bool,
                    body: vec![IrStmt::Return(Some(IrExpr::Bool(true)))],
                    is_public: true,
            is_async: false,
                }],
            }],
            routes: vec![],
        };
        let w = wat(&ir);
        assert!(w.contains("i32.const 1"), "true should be i32.const 1");
    }
}
