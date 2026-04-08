//! WASM backend — lowers an `IrModule` to WebAssembly Text format (WAT).
//!
//! # Usage
//! ```text
//! let wat = WasmCodegen::new().generate_wat(&ir_module)?;
//! std::fs::write("app.wat", &wat).unwrap();
//! // Assemble with: wat2wasm --enable-bulk-memory app.wat -o app.wasm
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
//! # Memory layout (generational semi-space GC)
//! ```text
//! [0 .. string_pool_end)        → null-terminated string literals (data section)
//! [heap_start .. page_1_start)  → unused padding (page-align nursery)
//! [page_1 .. page_3)            → nursery FROM-space  (128 KiB)
//! [page_3 .. page_5)            → nursery TO-space    (128 KiB)
//! [page_5 .. page_13)           → old generation      (512 KiB)
//! [page_13 .. page_14)          → shadow stack        (64 KiB)
//! [page_14 .. page_15)          → remembered set      (64 KiB)
//! ```
//! Total: 15 pages × 64 KiB = 960 KiB.
//!
//! # Object header (every heap-allocated object)
//! ```text
//! [ptr + 0] : i32  — type tag (index into shape map, set by $gc_alloc)
//! [ptr + 4] : i32  — forwarding pointer (0 = not moved; >0 = new address)
//! [ptr + 8..]: user fields (struct layout starts at HEADER_SIZE)
//! ```
//!
//! # GC algorithm
//! Minor GC uses **Cheney's copying collector**:
//!   1. Save `from_ptr` as the collection frontier.
//!   2. Reset `to_ptr = to_start`.
//!   3. Trace roots: shadow stack + remembered set.
//!   4. Scan to-space [to_start, to_ptr) calling `$gc_scan_object` on each.
//!   5. Swap from/to semi-spaces; reset `from_ptr = to_ptr`.
//!
//! Write barriers record old-gen → nursery pointers into the remembered set.
//! Shadow stack (push/pop at method entry/exit) tracks live GC roots on the
//! WASM call stack.
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
//!
//! # Sub-module layout
//! | File                | Responsibility                                  |
//! |---------------------|-------------------------------------------------|
//! | `wasm_codegen.rs`   | Types, `WatGen` core, `compile()`, public API   |
//! | `gc_runtime.rs`     | `emit_gc_globals`, `emit_gc_alloc`, `emit_gc_runtime` |
//! | `shape.rs`          | `emit_shape_map`                                |
//! | `method_codegen.rs` | `compile_class`, constructors, methods          |
//! | `expr_codegen.rs`   | `emit_stmt`, `emit_expr`, `infer_valtype`       |

use crate::domain::ir::*;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

// ── Sub-modules ────────────────────────────────────────────────────────────────

mod gc_runtime;
mod shape;
mod method_codegen;
mod expr_codegen;

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

// ── GC layout ─────────────────────────────────────────────────────────────────

/// Size of the object header prepended to every heap-allocated object.
///
/// ```text
/// [ptr + 0] : i32  — type tag
/// [ptr + 4] : i32  — forwarding pointer (0 = not moved)
/// [ptr + 8..]: user fields
/// ```
const HEADER_SIZE: u32 = 8;

/// Memory regions for the generational semi-space GC.
/// Offsets are bytes from the start of WASM linear memory.
struct GcLayout {
    nursery_from_start: u32,
    nursery_from_end:   u32,
    nursery_to_start:   u32,
    nursery_to_end:     u32,
    old_start:          u32,
    old_end:            u32,
    shadow_base:        u32,
    rset_base:          u32,
    total_pages:        u32,
}

impl GcLayout {
    const PAGE:          u32 = 65_536; // 64 KiB
    const NURSERY_PAGES: u32 = 2;      // 128 KiB per semi-space
    const OLD_GEN_PAGES: u32 = 8;      // 512 KiB
    const SHADOW_PAGES:  u32 = 1;      // 64 KiB
    const RSET_PAGES:    u32 = 1;      // 64 KiB

    fn new(heap_start: u32) -> Self {
        // Page-align the nursery so it starts at a clean 64 KiB boundary.
        let nursery_from_start = align_up(heap_start, Self::PAGE).max(Self::PAGE);
        let nursery_from_end   = nursery_from_start + Self::NURSERY_PAGES * Self::PAGE;
        let nursery_to_start   = nursery_from_end;
        let nursery_to_end     = nursery_to_start   + Self::NURSERY_PAGES * Self::PAGE;
        let old_start          = nursery_to_end;
        let old_end            = old_start          + Self::OLD_GEN_PAGES * Self::PAGE;
        let shadow_base        = old_end;
        let rset_base          = shadow_base        + Self::SHADOW_PAGES  * Self::PAGE;
        let total_pages        = (rset_base + Self::RSET_PAGES * Self::PAGE) / Self::PAGE;
        GcLayout {
            nursery_from_start,
            nursery_from_end,
            nursery_to_start,
            nursery_to_end,
            old_start,
            old_end,
            shadow_base,
            rset_base,
            total_pages,
        }
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
        // Field offsets start AFTER the 8-byte GC header.
        let mut offset = HEADER_SIZE;
        let mut slots = HashMap::new();
        for f in &cls.fields {
            let vt = ir_to_val(&f.ty);
            offset = align_up(offset, vt.byte_size());
            slots.insert(f.name.clone(), FieldSlot { offset, vt });
            offset += vt.byte_size();
        }
        StructLayout { slots, total_size: offset.max(HEADER_SIZE) }
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
    /// Type tag assigned to each class (index in module.classes).
    class_tags: HashMap<String, u32>,
    // ── GC v2: per-method shadow stack frame ───────────────────────────────────
    /// Map from local name → byte offset within `$__gc_frame` for i32 pointer locals.
    gc_ptr_frame: HashMap<String, u32>,
    /// Total size in bytes of the current function's shadow stack frame.
    gc_frame_size: u32,
    /// Return type of the current method (None = void).
    current_return_vt: Option<ValTy>,
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
            class_tags: HashMap::new(),
            gc_ptr_frame: HashMap::new(),
            gc_frame_size: 0,
            current_return_vt: None,
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

    // ── GC v2 frame helpers ───────────────────────────────────────────────────

    /// Return the WAT expression that reloads an i32 local through the GC
    /// forwarding-pointer check.
    fn reload_local(name: &str) -> String {
        format!("(call $gc_reload_if_forwarded (local.get ${name}))")
    }

    /// Emit the GC shadow stack frame epilogue: restore `$__gc_shadow_ptr` to
    /// the frame base.  Called before every `return` and at the end of any
    /// function body that has a non-zero frame.
    fn emit_frame_cleanup(&mut self) {
        if self.gc_frame_size > 0 {
            self.comment("Restore GC shadow stack");
            self.ln("(global.set $__gc_shadow_ptr (local.get $__gc_frame))");
        }
    }

    // ── Top-level entry ───────────────────────────────────────────────────────

    fn compile(&mut self, module: &IrModule) -> Result<(), WasmCodegenError> {
        // Phase 1 — collect strings, build layouts, assign type tags.
        self.pool.intern("#app");
        for (idx, cls) in module.classes.iter().enumerate() {
            self.class_tags.insert(cls.name.clone(), idx as u32);
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
        let gc = GcLayout::new(heap_start);

        // Snapshot per-class shape data before mutable borrows below.
        let shape_data: Vec<ClassShape> = module
            .classes
            .iter()
            .enumerate()
            .map(|(idx, cls)| {
                let size = self
                    .layouts
                    .get(&cls.name)
                    .map(|l| l.total_size)
                    .unwrap_or(HEADER_SIZE);
                let ptr_fields: Vec<(u32, String)> = self
                    .layouts
                    .get(&cls.name)
                    .map(|layout| {
                        let mut fields: Vec<(u32, String)> = layout
                            .slots
                            .iter()
                            .filter(|(_, slot)| slot.vt == ValTy::I32)
                            .map(|(name, slot)| (slot.offset, name.clone()))
                            .collect();
                        fields.sort_by_key(|(off, _)| *off);
                        fields
                    })
                    .unwrap_or_default();
                (idx as u32, cls.name.clone(), size, ptr_fields)
            })
            .collect();

        // Phase 2 — emit WAT.
        self.out
            .push_str(";; Generated by nexa-compiler — WASM target\n");
        self.out.push_str(
            ";; Assemble: wat2wasm --enable-bulk-memory app.wat -o app.wasm\n",
        );
        self.ln("(module");
        self.indent += 1;

        self.emit_memory(&gc)?;
        self.emit_imports();
        self.emit_gc_globals(&gc);
        self.emit_gc_alloc();
        self.emit_shape_map(&shape_data);
        self.emit_gc_runtime(&gc);

        for cls in &module.classes {
            self.compile_class(cls)?;
        }

        self.emit_start(module)?;

        self.indent -= 1;
        self.ln(")");
        Ok(())
    }

    // ── Memory + data section ─────────────────────────────────────────────────

    fn emit_memory(&mut self, gc: &GcLayout) -> Result<(), WasmCodegenError> {
        self.section(&format!(
            "Memory ({} pages = {} KiB — generational semi-space GC)",
            gc.total_pages,
            gc.total_pages * 64
        ));
        self.ln(&format!(
            "(memory (export \"memory\") {})",
            gc.total_pages
        ));

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
        self.comment(&format!(
            "Nursery from [{}, {}) | to [{}, {})",
            gc.nursery_from_start,
            gc.nursery_from_end,
            gc.nursery_to_start,
            gc.nursery_to_end
        ));
        self.comment(&format!(
            "Old gen [{}, {}) | shadow [{}, {}) | rset [{}, {})",
            gc.old_start,
            gc.old_end,
            gc.shadow_base,
            gc.shadow_base + GcLayout::SHADOW_PAGES * GcLayout::PAGE,
            gc.rset_base,
            gc.rset_base + GcLayout::RSET_PAGES * GcLayout::PAGE
        ));
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

    // ── Application entry point ───────────────────────────────────────────────

    fn emit_start(&mut self, module: &IrModule) -> Result<(), WasmCodegenError> {
        self.section("Application entry point");
        self.blank();
        self.ln("(func $_nexa_start (export \"_nexa_start\")");
        self.indent += 1;

        let app_sel_off = self.pool.index.get("#app").copied().unwrap_or(0);
        let main_win = module
            .routes
            .iter()
            .find(|r| r.path == "/")
            .map(|r| r.target.as_str());

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

// ── Shape data type alias ─────────────────────────────────────────────────────

/// Per-class shape descriptor: `(tag, class_name, total_size, pointer_fields)`.
/// `pointer_fields` is a list of `(field_offset, field_name)` for i32 fields.
type ClassShape = (u32, String, u32, Vec<(u32, String)>);

// ── Public API ────────────────────────────────────────────────────────────────

/// WASM backend: lowers an [`IrModule`] to WebAssembly Text format (WAT).
///
/// The output requires the **bulk-memory** WASM proposal for `memory.copy`.
/// Assemble with: `wat2wasm --enable-bulk-memory app.wat -o app.wasm`
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
            enums: vec![],
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
            enums: vec![],
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

    // ── Existing tests (backward compat) ─────────────────────────────────────

    #[test]
    fn output_starts_and_ends_as_module() {
        let w = wat(&counter_ir());
        assert!(w.contains("(module"), "should open with (module");
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
            enums: vec![],
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

    // ── New GC tests ──────────────────────────────────────────────────────────

    #[test]
    fn gc_alloc_function_present() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_alloc"), "$gc_alloc function missing");
        assert!(w.contains("$__gc_from_ptr"), "from-space pointer global missing");
    }

    #[test]
    fn gc_object_header_written_in_constructor() {
        let w = wat(&counter_ir());
        assert!(
            w.contains("i32.store (local.get $ptr) (local.get $tag)"),
            "type tag not written in header"
        );
        assert!(
            w.contains("i64.store offset=8"),
            "Int field should be at offset 8 (after header)"
        );
    }

    #[test]
    fn gc_minor_collect_present() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_minor_collect"), "$gc_minor_collect missing");
        assert!(w.contains("$gc_copy"), "$gc_copy missing");
        assert!(w.contains("memory.copy"), "memory.copy instruction missing");
    }

    #[test]
    fn gc_write_barrier_present() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_write_barrier"), "$gc_write_barrier missing");
        assert!(w.contains("$__gc_rset_ptr"), "remembered-set pointer missing");
    }

    #[test]
    fn gc_shadow_stack_present() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_shadow_push"), "$gc_shadow_push missing");
        assert!(w.contains("$gc_shadow_pop"), "$gc_shadow_pop missing");
        assert!(w.contains("$__gc_shadow_ptr"), "shadow stack pointer missing");
    }

    #[test]
    fn gc_self_reload_via_forwarded() {
        let w = wat(&counter_ir());
        assert!(
            w.contains("$gc_reload_if_forwarded"),
            "$gc_reload_if_forwarded missing"
        );
    }

    #[test]
    fn gc_semi_spaces_are_page_aligned() {
        let w = wat(&counter_ir());
        assert!(
            w.contains("65536"),
            "nursery from-space should start at page 1 (65536)"
        );
        assert!(w.contains("196608"), "nursery to-space start (196608) missing");
        assert!(w.contains("327680"), "old-gen start (327680) missing");
    }

    #[test]
    fn gc_shape_map_emits_object_size() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_object_size"), "$gc_object_size missing");
        assert!(w.contains("$gc_scan_object"), "$gc_scan_object missing");
    }

    // ── GC v2 tests — shadow-stack frame for let-bindings ─────────────────────

    fn gc_v2_ir() -> IrModule {
        IrModule {
            name: "GcV2Test".into(),
            server: None,
            enums: vec![],
            classes: vec![IrClass {
                name: "Node".into(),
                kind: IrClassKind::Class,
                is_public: true,
                fields: vec![],
                constructor_params: vec![],
                constructor_body: vec![],
                methods: vec![IrMethod {
                    name: "clone_node".into(),
                    params: vec![],
                    return_ty: IrType::Named("Node".into()),
                    body: vec![
                        IrStmt::Let {
                            name: "child".into(),
                            ty: IrType::Named("Node".into()),
                            init: IrExpr::Invoke {
                                callee: "Node".into(),
                                args: vec![],
                            },
                        },
                        IrStmt::Return(Some(IrExpr::Local("child".into()))),
                    ],
                    is_public: true,
                    is_async: false,
                }],
            }],
            routes: vec![],
        }
    }

    #[test]
    fn gc_v2_frame_local_declared() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(local $__gc_frame i32)"),
            "$__gc_frame local not declared"
        );
    }

    #[test]
    fn gc_v2_frame_setup_bumps_shadow_ptr() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(global.set $__gc_shadow_ptr"),
            "frame setup must bump $__gc_shadow_ptr"
        );
        assert!(
            w.contains("(local.set $__gc_frame (global.get $__gc_shadow_ptr))"),
            "frame base not saved to $__gc_frame"
        );
    }

    #[test]
    fn gc_v2_i32_let_writes_to_frame() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(local.get $__gc_frame)") && w.contains("(local.get $child)"),
            "i32 let-binding 'child' not written to GC frame"
        );
    }

    #[test]
    fn gc_v2_i32_local_read_goes_through_reload() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("$gc_reload_if_forwarded") && w.contains("(local.get $child)"),
            "i32 local 'child' not read through gc_reload_if_forwarded"
        );
    }

    #[test]
    fn gc_v2_frame_cleanup_restores_shadow_ptr() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(global.set $__gc_shadow_ptr (local.get $__gc_frame))"),
            "frame not restored on return"
        );
    }

    #[test]
    fn gc_v2_constructor_uses_frame_not_push_pop() {
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(i32.load (local.get $__gc_frame))"),
            "constructor must load $self back from GC frame"
        );
    }

    // ── CI WASM validation — wat2wasm + wasmtime ──────────────────────────────
    //
    // These tests call external tools (`wat2wasm` from wabt, `wasmtime`) to
    // validate that generated WAT assembles to a legal WASM binary.
    //
    // In CI the tools are installed before the test suite runs (see
    // `.github/workflows/snapshot.yml` step "Install wabt + wasmtime").
    // Locally the tests skip gracefully if the tools are absent.

    #[test]
    fn validate_wasm_binary_counter() {
        let wat_src = wat(&counter_ir());
        assert_wat_assembles_and_validates(&wat_src, "counter");
    }

    #[test]
    fn validate_wasm_binary_gc_v2() {
        let wat_src = wat(&gc_v2_ir());
        assert_wat_assembles_and_validates(&wat_src, "gc_v2");
    }

    /// Assemble `wat_src` with `wat2wasm --enable-bulk-memory`, then validate
    /// the resulting binary with `wasmtime validate`.  If either tool is absent
    /// the test is skipped (prints a notice and returns without failing).
    fn assert_wat_assembles_and_validates(wat_src: &str, label: &str) {
        use std::process::Command;

        // Skip if wat2wasm is not installed.
        if Command::new("wat2wasm").arg("--version").output().is_err() {
            eprintln!(
                "SKIP {label}: `wat2wasm` not found — install wabt to enable this test"
            );
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let wat_path  = dir.path().join(format!("{label}.wat"));
        let wasm_path = dir.path().join(format!("{label}.wasm"));

        std::fs::write(&wat_path, wat_src).expect("write .wat");

        let assemble = Command::new("wat2wasm")
            .args(["--enable-bulk-memory", "--enable-memory64"])
            .arg(&wat_path)
            .arg("-o")
            .arg(&wasm_path)
            .output()
            .expect("run wat2wasm");

        assert!(
            assemble.status.success(),
            "wat2wasm failed for '{label}':\n{}",
            String::from_utf8_lossy(&assemble.stderr)
        );

        // Validate binary if wasmtime is installed.
        if Command::new("wasmtime").arg("--version").output().is_ok() {
            let validate = Command::new("wasmtime")
                .arg("validate")
                .arg(&wasm_path)
                .output()
                .expect("run wasmtime");
            assert!(
                validate.status.success(),
                "wasmtime validate rejected '{label}':\n{}",
                String::from_utf8_lossy(&validate.stderr)
            );
        }
    }
}
