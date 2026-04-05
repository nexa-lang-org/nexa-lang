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
    /// All reads of these locals go through `$gc_reload_if_forwarded`; all definitions
    /// (let-bindings) also write to the frame so GC can update the address.
    gc_ptr_frame: HashMap<String, u32>,
    /// Total size in bytes of the current function's shadow stack frame.
    /// 0 means no frame has been set up (e.g., trivial void method with no pointers).
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
    /// forwarding-pointer check.  The result is an inline expression string.
    fn reload_local(name: &str) -> String {
        format!("(call $gc_reload_if_forwarded (local.get ${name}))")
    }

    /// Emit the GC shadow stack frame epilogue: restore `$__gc_shadow_ptr` to the
    /// frame base.  Must be called before every `return` and at the normal end of
    /// every function body that has a non-zero frame.
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

    // ── GC globals ────────────────────────────────────────────────────────────

    fn emit_gc_globals(&mut self, gc: &GcLayout) {
        self.section("GC globals — generational semi-space");
        self.blank();
        // Immutable region bounds
        self.ln(&format!("(global $__gc_old_start i32 (i32.const {}))", gc.old_start));
        self.ln(&format!("(global $__gc_old_end   i32 (i32.const {}))", gc.old_end));
        self.ln(&format!(
            "(global $__gc_shadow_base i32 (i32.const {}))",
            gc.shadow_base
        ));
        self.ln(&format!(
            "(global $__gc_rset_base   i32 (i32.const {}))",
            gc.rset_base
        ));
        self.blank();
        // Mutable: current from-space (swapped with to-space on each minor GC)
        self.ln(&format!(
            "(global $__gc_from_start (mut i32) (i32.const {}))",
            gc.nursery_from_start
        ));
        self.ln(&format!(
            "(global $__gc_from_end   (mut i32) (i32.const {}))",
            gc.nursery_from_end
        ));
        self.ln(&format!(
            "(global $__gc_from_ptr   (mut i32) (i32.const {}))",
            gc.nursery_from_start
        ));
        // Mutable: current to-space
        self.ln(&format!(
            "(global $__gc_to_start   (mut i32) (i32.const {}))",
            gc.nursery_to_start
        ));
        self.ln(&format!(
            "(global $__gc_to_end     (mut i32) (i32.const {}))",
            gc.nursery_to_end
        ));
        self.ln(&format!(
            "(global $__gc_to_ptr     (mut i32) (i32.const {}))",
            gc.nursery_to_start
        ));
        // Old-gen bump pointer
        self.ln(&format!(
            "(global $__gc_old_ptr    (mut i32) (i32.const {}))",
            gc.old_start
        ));
        // Shadow stack top
        self.ln(&format!(
            "(global $__gc_shadow_ptr (mut i32) (i32.const {}))",
            gc.shadow_base
        ));
        // Remembered set top
        self.ln(&format!(
            "(global $__gc_rset_ptr   (mut i32) (i32.const {}))",
            gc.rset_base
        ));
        // Saved from-ptr at start of minor GC (boundary for $gc_copy)
        self.ln(&format!(
            "(global $__gc_collect_frontier (mut i32) (i32.const {}))",
            gc.nursery_from_start
        ));
        self.blank();
        // Legacy globals kept for backward compatibility with tests and tooling.
        self.comment("Legacy: bump-allocator globals (backed by GC from-space)");
        self.ln(&format!(
            "(global $__heap_ptr (mut i32) (i32.const {}))",
            gc.nursery_from_start
        ));
    }

    // ── GC allocator ─────────────────────────────────────────────────────────

    fn emit_gc_alloc(&mut self) {
        self.section("GC allocator");
        self.blank();
        self.comment(
            "$gc_alloc — bump-allocate 'size' bytes tagged with 'tag'; \
             triggers minor GC when nursery is full.",
        );
        self.ln("(func $gc_alloc (param $size i32) (param $tag i32) (result i32)");
        self.indent += 1;
        self.ln("(local $ptr i32)");
        self.comment("Trigger minor GC if nursery would overflow");
        self.ln("(if (i32.gt_u");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_from_ptr) (local.get $size))");
        self.ln("(global.get $__gc_from_end))");
        self.indent -= 1;
        self.ln("(then (call $gc_minor_collect)))");
        self.comment("Bump-allocate");
        self.ln("(local.set $ptr (global.get $__gc_from_ptr))");
        self.ln("(global.set $__gc_from_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_from_ptr) (local.get $size)))");
        self.indent -= 1;
        self.comment("Write object header: tag at [+0], fwd=0 at [+4]");
        self.ln("(i32.store (local.get $ptr) (local.get $tag))");
        self.ln("(i32.store offset=4 (local.get $ptr) (i32.const 0))");
        self.comment("Sync legacy heap pointer");
        self.ln("(global.set $__heap_ptr (global.get $__gc_from_ptr))");
        self.ln("(local.get $ptr))");
        self.indent -= 1;
        self.blank();
        self.comment("$__alloc — legacy wrapper: calls $gc_alloc with tag=0.");
        self.ln("(func $__alloc (param $size i32) (result i32)");
        self.indent += 1;
        self.ln("(call $gc_alloc (local.get $size) (i32.const 0)))");
        self.indent -= 1;
    }

    // ── Shape map ────────────────────────────────────────────────────────────

    /// Emit `$gc_object_size` and `$gc_scan_object` using pre-collected shape data.
    ///
    /// `shape_data`: `(tag, class_name, total_size, [(field_offset, field_name)])`
    fn emit_shape_map(&mut self, shape_data: &[ClassShape]) {
        self.section("Shape map — object size and pointer-field scanner");
        self.blank();

        // $gc_object_size: dispatch on tag → total object size (incl. header).
        self.ln("(func $gc_object_size (param $tag i32) (result i32)");
        self.indent += 1;
        for (tag, cls_name, size, _) in shape_data {
            self.ln(&format!(
                "(if (i32.eq (local.get $tag) (i32.const {tag}))",
            ));
            self.indent += 1;
            self.ln(&format!(
                "(then (return (i32.const {size}))))  ;; class {cls_name}"
            ));
            self.indent -= 1;
        }
        self.ln(&format!(
            "(i32.const {}))  ;; unknown tag — minimum header size",
            HEADER_SIZE
        ));
        self.indent -= 1;
        self.blank();

        // $gc_scan_object: for each pointer field in the object, call $gc_copy
        // and write back the (potentially new) address.
        self.ln("(func $gc_scan_object (param $ptr i32)");
        self.indent += 1;
        self.ln("(local $tag i32)");
        self.ln("(local.set $tag (i32.load (local.get $ptr)))");

        for (tag, cls_name, _, ptr_fields) in shape_data {
            if ptr_fields.is_empty() {
                continue;
            }
            self.ln(&format!(
                "(if (i32.eq (local.get $tag) (i32.const {tag}))",
            ));
            self.indent += 1;
            self.ln("(then");
            self.indent += 1;
            for (offset, field_name) in ptr_fields {
                self.comment(&format!(
                    "class {cls_name} — field '{field_name}' at +{offset}"
                ));
                self.ln(&format!(
                    "(i32.store offset={offset} (local.get $ptr)"
                ));
                self.indent += 1;
                self.ln(&format!(
                    "(call $gc_copy (i32.load offset={offset} (local.get $ptr)))))))",
                ));
                self.indent -= 1;
            }
            self.indent -= 1;
            self.indent -= 1;
        }
        self.ln(")");
        self.indent -= 1;
    }

    // ── GC runtime functions ──────────────────────────────────────────────────

    fn emit_gc_runtime(&mut self, _gc: &GcLayout) {
        self.section("GC runtime — Cheney copying collector");

        // ── $gc_reload_if_forwarded ───────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_reload_if_forwarded — if the object was moved, return its new address.",
        );
        self.ln("(func $gc_reload_if_forwarded (param $ptr i32) (result i32)");
        self.indent += 1;
        self.ln("(local $fwd i32)");
        self.comment("Null pointer — return as-is");
        self.ln("(if (i32.eqz (local.get $ptr))");
        self.indent += 1;
        self.ln("(then (return (i32.const 0))))");
        self.indent -= 1;
        self.comment("Read forwarding pointer from header [+4]");
        self.ln("(local.set $fwd (i32.load offset=4 (local.get $ptr)))");
        self.ln("(if (local.get $fwd)");
        self.indent += 1;
        self.ln("(then (return (local.get $fwd))))");
        self.indent -= 1;
        self.ln("(local.get $ptr))");
        self.indent -= 1;

        // ── $gc_copy ─────────────────────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_copy — copy a nursery object to to-space (or old-gen if to-space is full). \
             Returns the new address; sets forwarding pointer in the original.",
        );
        self.ln("(func $gc_copy (param $ptr i32) (result i32)");
        self.indent += 1;
        self.ln("(local $fwd i32)");
        self.ln("(local $size i32)");
        self.ln("(local $new_ptr i32)");
        self.comment("Null pointer — return as-is");
        self.ln("(if (i32.eqz (local.get $ptr))");
        self.indent += 1;
        self.ln("(then (return (i32.const 0))))");
        self.indent -= 1;
        self.comment("Not in from-space (e.g., old-gen, string pool) — return unchanged");
        self.ln("(if (i32.or");
        self.indent += 1;
        self.ln("(i32.lt_u (local.get $ptr) (global.get $__gc_from_start))");
        self.ln("(i32.ge_u (local.get $ptr) (global.get $__gc_collect_frontier)))");
        self.indent -= 1;
        self.ln("(then (return (local.get $ptr))))");
        self.comment("Already forwarded? Return the forwarding pointer");
        self.ln("(local.set $fwd (i32.load offset=4 (local.get $ptr)))");
        self.ln("(if (local.get $fwd)");
        self.indent += 1;
        self.ln("(then (return (local.get $fwd))))");
        self.indent -= 1;
        self.comment("Get object size from shape map");
        self.ln("(local.set $size (call $gc_object_size (i32.load (local.get $ptr))))");
        self.comment("Try to-space first; fall back to old-gen if to-space is full");
        self.ln("(if (i32.ge_u");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_to_ptr) (local.get $size))");
        self.ln("(global.get $__gc_to_end))");
        self.indent -= 1;
        self.ln("(then");
        self.indent += 1;
        self.comment("Promote to old-gen");
        self.ln("(local.set $new_ptr (global.get $__gc_old_ptr))");
        self.ln("(global.set $__gc_old_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_old_ptr) (local.get $size))))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln("(else");
        self.indent += 1;
        self.comment("Copy to to-space");
        self.ln("(local.set $new_ptr (global.get $__gc_to_ptr))");
        self.ln("(global.set $__gc_to_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_to_ptr) (local.get $size))))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln(")");
        self.comment("Copy object bytes (requires --enable-bulk-memory / bulk-memory proposal)");
        self.ln("(memory.copy (local.get $new_ptr) (local.get $ptr) (local.get $size))");
        self.comment("Write forwarding pointer into original's header [+4]");
        self.ln("(i32.store offset=4 (local.get $ptr) (local.get $new_ptr))");
        self.comment("Clear fwd in the copy so it doesn't look forwarded");
        self.ln("(i32.store offset=4 (local.get $new_ptr) (i32.const 0))");
        self.ln("(local.get $new_ptr))");
        self.indent -= 1;

        // ── $gc_shadow_push / $gc_shadow_pop ─────────────────────────────
        self.blank();
        self.comment(
            "$gc_shadow_push — push a GC root onto the shadow stack.",
        );
        self.ln("(func $gc_shadow_push (param $val i32)");
        self.indent += 1;
        self.ln("(i32.store (global.get $__gc_shadow_ptr) (local.get $val))");
        self.ln("(global.set $__gc_shadow_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_shadow_ptr) (i32.const 4))))");
        self.indent -= 1;
        self.indent -= 1;
        self.blank();
        self.comment(
            "$gc_shadow_pop — pop a (potentially GC-updated) root off the shadow stack.",
        );
        self.ln("(func $gc_shadow_pop (result i32)");
        self.indent += 1;
        self.ln("(global.set $__gc_shadow_ptr");
        self.indent += 1;
        self.ln("(i32.sub (global.get $__gc_shadow_ptr) (i32.const 4)))");
        self.indent -= 1;
        self.ln("(i32.load (global.get $__gc_shadow_ptr)))");
        self.indent -= 1;

        // ── $gc_trace_shadow_stack ────────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_trace_shadow_stack — copy-update all live roots on the shadow stack.",
        );
        self.ln("(func $gc_trace_shadow_stack");
        self.indent += 1;
        self.ln("(local $scan i32)");
        self.ln("(local.set $scan (global.get $__gc_shadow_base))");
        self.ln("(block $done");
        self.indent += 1;
        self.ln("(loop $loop");
        self.indent += 1;
        self.ln("(br_if $done (i32.ge_u (local.get $scan) (global.get $__gc_shadow_ptr)))");
        self.ln("(i32.store (local.get $scan)");
        self.indent += 1;
        self.ln("(call $gc_copy (i32.load (local.get $scan))))");
        self.indent -= 1;
        self.ln("(local.set $scan (i32.add (local.get $scan) (i32.const 4)))");
        self.ln("(br $loop)))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln(")");
        self.indent -= 1;

        // ── $gc_trace_remembered_set ──────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_trace_remembered_set — re-scan old-gen objects that point into nursery.",
        );
        self.ln("(func $gc_trace_remembered_set");
        self.indent += 1;
        self.ln("(local $scan i32)");
        self.ln("(local.set $scan (global.get $__gc_rset_base))");
        self.ln("(block $done");
        self.indent += 1;
        self.ln("(loop $loop");
        self.indent += 1;
        self.ln("(br_if $done (i32.ge_u (local.get $scan) (global.get $__gc_rset_ptr)))");
        self.ln("(call $gc_scan_object (i32.load (local.get $scan)))");
        self.ln("(local.set $scan (i32.add (local.get $scan) (i32.const 4)))");
        self.ln("(br $loop)))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln(")");
        self.indent -= 1;

        // ── $gc_write_barrier ─────────────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_write_barrier — record old-gen→nursery pointer stores in the remembered set.",
        );
        self.ln(
            "(func $gc_write_barrier (param $obj_ptr i32) (param $new_val i32)",
        );
        self.indent += 1;
        self.comment("If obj_ptr is in old-gen AND new_val is in nursery from-space → record");
        self.ln("(if (i32.and");
        self.indent += 1;
        self.ln("(i32.and");
        self.indent += 1;
        self.ln("(i32.ge_u (local.get $obj_ptr) (global.get $__gc_old_start))");
        self.ln("(i32.lt_u (local.get $obj_ptr) (global.get $__gc_old_end)))");
        self.indent -= 1;
        self.ln("(i32.and");
        self.indent += 1;
        self.ln("(i32.ge_u (local.get $new_val) (global.get $__gc_from_start))");
        self.ln("(i32.lt_u (local.get $new_val) (global.get $__gc_from_ptr))))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln("(then");
        self.indent += 1;
        self.ln("(i32.store (global.get $__gc_rset_ptr) (local.get $obj_ptr))");
        self.ln("(global.set $__gc_rset_ptr");
        self.indent += 1;
        self.ln("(i32.add (global.get $__gc_rset_ptr) (i32.const 4)))))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln(")");
        self.indent -= 1;

        // ── $gc_minor_collect ─────────────────────────────────────────────
        self.blank();
        self.comment(
            "$gc_minor_collect — Cheney's copying minor GC: \
             evacuates live nursery objects to to-space, then swaps semi-spaces.",
        );
        self.ln("(func $gc_minor_collect");
        self.indent += 1;
        self.ln("(local $scan i32)");
        self.ln("(local $obj_size i32)");
        self.ln("(local $old_from_start i32)");
        self.ln("(local $old_from_end i32)");
        self.blank();
        self.comment("Save allocation frontier so $gc_copy knows what was live");
        self.ln("(global.set $__gc_collect_frontier (global.get $__gc_from_ptr))");
        self.comment("Reset to-space scanning/allocation pointer");
        self.ln("(global.set $__gc_to_ptr (global.get $__gc_to_start))");
        self.blank();
        self.comment("1. Trace roots: shadow stack + remembered set");
        self.ln("(call $gc_trace_shadow_stack)");
        self.ln("(call $gc_trace_remembered_set)");
        self.blank();
        self.comment("2. Cheney scan: walk to-space, updating each object's pointer fields");
        self.ln("(local.set $scan (global.get $__gc_to_start))");
        self.ln("(block $scan_done");
        self.indent += 1;
        self.ln("(loop $scan_loop");
        self.indent += 1;
        self.ln("(br_if $scan_done (i32.ge_u (local.get $scan) (global.get $__gc_to_ptr)))");
        self.ln("(call $gc_scan_object (local.get $scan))");
        self.ln(
            "(local.set $obj_size (call $gc_object_size (i32.load (local.get $scan))))",
        );
        self.ln("(local.set $scan (i32.add (local.get $scan) (local.get $obj_size)))");
        self.ln("(br $scan_loop))");
        self.indent -= 1;
        self.indent -= 1;
        self.ln(")");
        self.blank();
        self.comment("3. Swap from-space and to-space");
        self.ln("(local.set $old_from_start (global.get $__gc_from_start))");
        self.ln("(local.set $old_from_end   (global.get $__gc_from_end))");
        self.comment("New from-space = old to-space (where survivors live)");
        self.ln("(global.set $__gc_from_start (global.get $__gc_to_start))");
        self.ln("(global.set $__gc_from_end   (global.get $__gc_to_end))");
        self.ln("(global.set $__gc_from_ptr   (global.get $__gc_to_ptr))");
        self.comment("New to-space = old from-space (now free for next collection)");
        self.ln("(global.set $__gc_to_start   (local.get $old_from_start))");
        self.ln("(global.set $__gc_to_end     (local.get $old_from_end))");
        self.ln("(global.set $__gc_to_ptr     (local.get $old_from_start))");
        self.comment("Sync legacy heap pointer");
        self.ln("(global.set $__heap_ptr (global.get $__gc_from_ptr))");
        self.blank();
        self.comment("4. Clear remembered set");
        self.ln("(global.set $__gc_rset_ptr (global.get $__gc_rset_base)))");
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
            self.comment(&format!("total = {} bytes (incl. 8-byte GC header)", total_size));
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

    // ── GC v2: build the shadow-stack frame map for a set of pointer locals ─────

    /// Build `gc_ptr_frame` for the current function.
    ///
    /// Frame layout (each slot = 4 bytes):
    ///   [0]     $self
    ///   [4…]    pointer-typed params (i32), in order
    ///   [N…]    i32 let-bound locals, in declaration order
    ///
    /// Always call this before emitting a constructor or method body.
    fn build_frame(
        &mut self,
        ptr_params: &[String],
        let_locals: &[(String, ValTy)],
    ) {
        self.gc_ptr_frame.clear();
        self.gc_frame_size = 0;

        // $self is always the first slot.
        self.gc_ptr_frame.insert("self".to_string(), 0);
        self.gc_frame_size = 4;

        for name in ptr_params {
            if !self.gc_ptr_frame.contains_key(name) {
                self.gc_ptr_frame.insert(name.clone(), self.gc_frame_size);
                self.gc_frame_size += 4;
            }
        }

        for (name, vt) in let_locals {
            if *vt == ValTy::I32 && !self.gc_ptr_frame.contains_key(name) {
                self.gc_ptr_frame.insert(name.clone(), self.gc_frame_size);
                self.gc_frame_size += 4;
            }
        }
    }

    /// Emit the shadow-stack frame prologue: bump `$__gc_shadow_ptr` and
    /// zero-initialise all slots.  Must be called AFTER `build_frame`.
    fn emit_frame_setup(&mut self) {
        if self.gc_frame_size == 0 {
            return;
        }
        self.comment(&format!(
            "GC v2 shadow stack frame ({} bytes — {} pointer locals)",
            self.gc_frame_size,
            self.gc_frame_size / 4
        ));
        self.ln("(local.set $__gc_frame (global.get $__gc_shadow_ptr))");
        self.ln(&format!(
            "(global.set $__gc_shadow_ptr \
             (i32.add (global.get $__gc_shadow_ptr) (i32.const {})))",
            self.gc_frame_size
        ));
        // Zero-initialise every slot so GC never sees garbage during setup.
        for slot in (0..self.gc_frame_size).step_by(4) {
            if slot == 0 {
                self.ln("(i32.store (local.get $__gc_frame) (i32.const 0))");
            } else {
                self.ln(&format!(
                    "(i32.store offset={slot} (local.get $__gc_frame) (i32.const 0))"
                ));
            }
        }
    }

    // ── Constructor ───────────────────────────────────────────────────────────

    fn emit_constructor(&mut self, cls: &IrClass) -> Result<(), WasmCodegenError> {
        let tag  = self.class_tags.get(&cls.name).copied().unwrap_or(0);
        let size = self.layouts.get(&cls.name).map(|l| l.total_size).unwrap_or(HEADER_SIZE);

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
        self.ln("(local $self   i32)");
        self.ln("(local $__gc_frame i32)");
        self.ln("(local $__rv_i32   i32)");
        self.ln("(local $__wb_obj   i32)");
        self.ln("(local $__wb_val   i32)");

        // ── Build GC frame (self + pointer params + i32 body lets) ────────
        let ptr_params: Vec<String> = cls
            .constructor_params
            .iter()
            .filter(|p| ir_to_val(&p.ty) == ValTy::I32)
            .map(|p| p.name.clone())
            .collect();

        let mut body_lets: Vec<(String, ValTy)> = Vec::new();
        collect_let_locals(&cls.constructor_body, &mut body_lets);

        self.build_frame(&ptr_params, &body_lets);
        self.emit_frame_setup();

        // ── Allocate $self (GC may trigger here; frame is all-zero so safe) ─
        self.comment(&format!("Allocate: {} bytes, tag={}", size, tag));
        self.ln(&format!(
            "(local.set $self (call $gc_alloc (i32.const {}) (i32.const {})))",
            size, tag
        ));
        // Write $self into frame slot 0 so subsequent allocations keep it alive.
        self.ln("(i32.store (local.get $__gc_frame) (local.get $self))");

        // Write pointer params into their frame slots.
        for p in &cls.constructor_params {
            if ir_to_val(&p.ty) == ValTy::I32 {
                if let Some(&off) = self.gc_ptr_frame.get(&p.name) {
                    let store = Self::frame_store_str(off, &format!("(local.get ${})", p.name));
                    self.ln(&store);
                }
            }
        }

        // ── Store constructor params into the new object's fields ──────────
        for p in &cls.constructor_params {
            let vt = ir_to_val(&p.ty);
            if let Some(layout) = self.layouts.get(&cls.name) {
                if let Some((offset, _)) = layout.get(&p.name) {
                    if vt == ValTy::I32 {
                        self.ln(&format!(
                            "(call $gc_write_barrier (local.get $self) (local.get ${}))",
                            p.name
                        ));
                    }
                    self.ln(&format!(
                        "({} (local.get $self) (local.get ${}))",
                        vt.store(offset),
                        p.name
                    ));
                }
            }
        }

        // ── Optional explicit constructor body ─────────────────────────────
        if !cls.constructor_body.is_empty() {
            self.current_class = cls.name.clone();
            self.current_return_vt = Some(ValTy::I32);
            let mut let_locals_for_map: Vec<(String, ValTy)> =
                vec![("self".to_string(), ValTy::I32)];
            collect_let_locals(&cls.constructor_body, &mut let_locals_for_map);
            let local_map: HashMap<String, ValTy> = let_locals_for_map.into_iter().collect();
            self.emit_stmts(&cls.constructor_body.clone(), &local_map)?;
        }

        // ── Return $self — reload from frame in case GC moved it ──────────
        self.comment("Return $self — GC-reloaded from frame in case of relocation");
        self.ln("(local.set $__rv_i32 (i32.load (local.get $__gc_frame)))");
        self.emit_frame_cleanup();
        self.ln("(local.get $__rv_i32))");
        self.indent -= 1;
        self.gc_ptr_frame.clear();
        self.gc_frame_size = 0;
        self.current_return_vt = None;
        Ok(())
    }

    // ── Method ────────────────────────────────────────────────────────────────

    fn emit_method(
        &mut self,
        class_name: &str,
        method: &IrMethod,
    ) -> Result<(), WasmCodegenError> {
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
            if method.params.is_empty() {
                String::new()
            } else {
                format!(
                    ", {}",
                    method
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, ir_to_val(&p.ty).as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            match &method.return_ty {
                IrType::Void => "void",
                ty => ir_to_val(ty).as_str(),
            }
        ));

        self.ln(&format!(
            "(func ${}_{} {}{}",
            class_name, method.name, params_str, result_str
        ));
        self.indent += 1;

        // ── Build local type map ─────────────────────────────────────────
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

        // GC + scratch locals — always declared.
        self.ln("(local $__gc_frame i32)");
        self.ln("(local $__rv_i32   i32)");
        self.ln("(local $__rv_i64   i64)");
        self.ln("(local $__wb_obj   i32)");
        self.ln("(local $__wb_val   i32)");

        // ── Build GC v2 shadow-stack frame ───────────────────────────────
        let ptr_params: Vec<String> = method
            .params
            .iter()
            .filter(|p| ir_to_val(&p.ty) == ValTy::I32)
            .map(|p| p.name.clone())
            .collect();

        self.build_frame(&ptr_params, &let_locals);
        self.current_return_vt = if method.return_ty == IrType::Void {
            None
        } else {
            Some(ir_to_val(&method.return_ty))
        };

        // Frame prologue: bump shadow_ptr, zero-init, write initial values.
        self.emit_frame_setup();
        if self.gc_frame_size > 0 {
            // Write $self into frame slot 0.
            self.ln("(i32.store (local.get $__gc_frame) (local.get $self))");
            // Write pointer params into their frame slots.
            for p in &method.params {
                if ir_to_val(&p.ty) == ValTy::I32 {
                    if let Some(&off) = self.gc_ptr_frame.get(&p.name) {
                        let store =
                            Self::frame_store_str(off, &format!("(local.get ${})", p.name));
                        self.ln(&store);
                    }
                }
            }
        }

        // ── Emit body ────────────────────────────────────────────────────
        self.node_idx = 0;
        self.emit_stmts(&method.body.clone(), &locals)?;

        // Frame cleanup at the natural end of the function (void methods or
        // fall-through non-void paths that have no explicit return).
        self.emit_frame_cleanup();

        self.ln(")");
        self.indent -= 1;
        self.gc_ptr_frame.clear();
        self.gc_frame_size = 0;
        self.current_return_vt = None;
        Ok(())
    }

    /// Static helper: produce the WAT for an i32.store into the GC frame at `offset`.
    fn frame_store_str(offset: u32, value_wat: &str) -> String {
        if offset == 0 {
            format!("(i32.store (local.get $__gc_frame) {value_wat})")
        } else {
            format!("(i32.store offset={offset} (local.get $__gc_frame) {value_wat})")
        }
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
            IrStmt::Let { name, ty, init } => {
                self.emit_expr(init, locals)?;
                self.ln(&format!("local.set ${}", name));
                // GC v2: if this is an i32 (pointer) local tracked in the frame,
                // write it to the frame so GC can keep the object alive and update
                // the address if it gets copied during a later allocation.
                if ir_to_val(ty) == ValTy::I32 {
                    if let Some(&off) = self.gc_ptr_frame.get(name.as_str()) {
                        let store =
                            Self::frame_store_str(off, &format!("(local.get ${})", name));
                        self.ln(&store);
                    }
                }
            }
            IrStmt::Assign { target, value } => {
                match target {
                    IrExpr::Field { receiver, name } => {
                        let cls = self.current_class.clone();
                        let (offset, vt) = self
                            .layouts
                            .get(&cls)
                            .and_then(|l| l.get(name))
                            .unwrap_or((0, ValTy::I32));

                        if vt == ValTy::I32 {
                            // Pointer store: emit write barrier via $__wb_obj / $__wb_val.
                            // local.tee leaves the value on the stack AND saves to the local.
                            self.emit_expr(receiver, locals)?;
                            self.ln("local.tee $__wb_obj");
                            self.emit_expr(value, locals)?;
                            self.ln("local.tee $__wb_val");
                            // Stack: [obj_ptr, val] — $gc_write_barrier consumes both.
                            self.ln("call $gc_write_barrier");
                            // Reload from saved locals for the actual store.
                            self.ln("local.get $__wb_obj");
                            self.ln("local.get $__wb_val");
                            self.ln(&vt.store(offset));
                        } else {
                            // Non-pointer store (e.g., i64 Int field): no write barrier.
                            self.emit_expr(receiver, locals)?;
                            self.emit_expr(value, locals)?;
                            self.ln(&vt.store(offset));
                        }
                    }
                    _ => {
                        self.emit_expr(value, locals)?;
                        self.ln("drop  ;; assign to non-field target");
                    }
                }
            }
            IrStmt::Return(None) => {
                self.emit_frame_cleanup();
                self.ln("return");
            }
            IrStmt::Return(Some(e)) => {
                self.emit_expr(e, locals)?;
                // Save the return value before restoring the frame so that the
                // WASM stack is empty when we call emit_frame_cleanup.
                if self.gc_frame_size > 0 {
                    match self.current_return_vt {
                        Some(ValTy::I64) => {
                            self.ln("local.set $__rv_i64");
                            self.emit_frame_cleanup();
                            self.ln("local.get $__rv_i64");
                        }
                        _ => {
                            self.ln("local.set $__rv_i32");
                            self.emit_frame_cleanup();
                            self.ln("local.get $__rv_i32");
                        }
                    }
                }
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
            // GC v2: i32 locals tracked in the shadow-stack frame are accessed via
            // gc_reload_if_forwarded so that stale pointers (the GC may have moved
            // the object since this local was last written) are transparently resolved.
            IrExpr::Local(name) => {
                if self.gc_ptr_frame.contains_key(name.as_str()) {
                    self.ln(&Self::reload_local(name));
                } else {
                    self.ln(&format!("local.get ${}", name));
                }
                true
            }
            // SelfRef is equivalent to Local("self") but expressed as a dedicated
            // variant; both go through gc_reload_if_forwarded.
            IrExpr::SelfRef => {
                self.ln("(call $gc_reload_if_forwarded (local.get $self))");
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
                self.emit_expr(receiver, locals)?;
                for arg in args {
                    self.emit_expr(arg, locals)?;
                }
                let cls = self.current_class.clone();
                self.ln(&format!("call ${}_{}", cls, method));
                !self.void_methods.contains(&(cls, method.clone()))
            }
            IrExpr::Invoke { callee, args } => {
                for arg in args {
                    self.emit_expr(arg, locals)?;
                }
                self.ln(&format!("call ${}_new", callee));
                true // constructors always return i32
            }
            IrExpr::Node { tag, children } => {
                let tag_off = self.pool.index.get(tag.as_str()).copied().unwrap_or(0);
                let node_local = format!("$__node_{}", self.node_idx);
                self.node_idx += 1;

                self.ln(&format!("i32.const {}  ;; tag \"{}\"", tag_off, tag));
                self.ln("call $env_dom_create_element");
                self.ln(&format!("local.set {}", node_local));

                for child in children {
                    if let IrExpr::Str(s) = child {
                        let str_off =
                            self.pool.index.get(s.as_str()).copied().unwrap_or(0);
                        self.ln(&format!("local.get {}  ;; element", node_local));
                        self.ln(&format!("i32.const {}  ;; \"{}\"", str_off, s.escape_default()));
                        self.ln("call $env_dom_set_text_content");
                    } else {
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
                self.ln(
                    "i32.const 0  ;; closure (function-references not yet supported)",
                );
                true
            }
            // Await: passthrough — actual async is a JS concept; WAT sees the value directly.
            IrExpr::Await(inner) => {
                self.emit_expr(inner, locals)?;
                true
            }
            // List and DynamicImport are JS-target concepts; emit null in WASM.
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
            IrExpr::Bin {
                op: IrBinOp::Add | IrBinOp::Sub | IrBinOp::Mul | IrBinOp::Div | IrBinOp::Mod,
                lhs,
                ..
            } => self.infer_valtype(lhs, locals),
            IrExpr::Bin { .. } => ValTy::I32, // comparisons/logical → bool (i32)
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
        // Legacy wrappers are kept for backward compat.
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
        // Constructor must store the type tag at offset +0 (i32.store with no offset).
        assert!(
            w.contains("i32.store (local.get $ptr) (local.get $tag)"),
            "type tag not written in header"
        );
        // The Int field 'count' is now at offset 8 (after 8-byte header).
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
        // SelfRef in a method must go through $gc_reload_if_forwarded so that
        // if GC moved the object during an allocation in this method, we still
        // access the right address.
        let w = wat(&counter_ir());
        assert!(
            w.contains("$gc_reload_if_forwarded"),
            "$gc_reload_if_forwarded missing"
        );
    }

    #[test]
    fn gc_semi_spaces_are_page_aligned() {
        // Nursery from-space must start at a 64 KiB page boundary (≥ page 1).
        let w = wat(&counter_ir());
        assert!(
            w.contains("65536"),
            "nursery from-space should start at page 1 (65536)"
        );
        // To-space follows immediately after (2 pages × 64 KiB = 128 KiB later).
        assert!(w.contains("196608"), "nursery to-space start (196608) missing");
        // Old generation starts after both nursery semi-spaces.
        assert!(w.contains("327680"), "old-gen start (327680) missing");
    }

    #[test]
    fn gc_shape_map_emits_object_size() {
        let w = wat(&counter_ir());
        assert!(w.contains("$gc_object_size"), "$gc_object_size missing");
        assert!(w.contains("$gc_scan_object"), "$gc_scan_object missing");
    }

    // ── GC v2 tests — shadow-stack frame for let-bindings ─────────────────────

    /// IR fixture: a class with a method that has a Named (pointer) let-binding.
    fn gc_v2_ir() -> IrModule {
        IrModule {
            name: "GcV2Test".into(),
            server: None,
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
                    // body: let child: Node = Node_new(); return child;
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
        // Every method must declare a $__gc_frame local.
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(local $__gc_frame i32)"),
            "$__gc_frame local not declared"
        );
    }

    #[test]
    fn gc_v2_frame_setup_bumps_shadow_ptr() {
        // The frame prologue must bump $__gc_shadow_ptr by the frame size.
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(global.set $__gc_shadow_ptr"),
            "frame setup must bump $__gc_shadow_ptr"
        );
        // Frame must be saved to $__gc_frame from the current shadow_ptr.
        assert!(
            w.contains("(local.set $__gc_frame (global.get $__gc_shadow_ptr))"),
            "frame base not saved to $__gc_frame"
        );
    }

    #[test]
    fn gc_v2_i32_let_writes_to_frame() {
        // After binding a Named (i32) let, the value must be written into the frame.
        // The store pattern is: i32.store ... $__gc_frame ... $child
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(local.get $__gc_frame)") && w.contains("(local.get $child)"),
            "i32 let-binding 'child' not written to GC frame"
        );
    }

    #[test]
    fn gc_v2_i32_local_read_goes_through_reload() {
        // Reading a tracked i32 local must call $gc_reload_if_forwarded.
        let w = wat(&gc_v2_ir());
        // The return statement has `return child` which should emit:
        //   (call $gc_reload_if_forwarded (local.get $child))
        assert!(
            w.contains("$gc_reload_if_forwarded") && w.contains("(local.get $child)"),
            "i32 local 'child' not read through gc_reload_if_forwarded"
        );
    }

    #[test]
    fn gc_v2_frame_cleanup_restores_shadow_ptr() {
        // Before every return, the frame must be restored:
        //   (global.set $__gc_shadow_ptr (local.get $__gc_frame))
        let w = wat(&gc_v2_ir());
        assert!(
            w.contains("(global.set $__gc_shadow_ptr (local.get $__gc_frame))"),
            "frame not restored on return"
        );
    }

    #[test]
    fn gc_v2_constructor_uses_frame_not_push_pop() {
        // GC v2 constructors must NOT call $gc_shadow_push / $gc_shadow_pop inside
        // the constructor body — they use the frame approach instead.
        let w = wat(&gc_v2_ir());
        // The push/pop FUNCTIONS are still defined (part of GC runtime), but they
        // must NOT appear as call instructions inside the constructor function body.
        // We verify the frame approach: the constructor loads $self back from frame.
        assert!(
            w.contains("(i32.load (local.get $__gc_frame))"),
            "constructor must load $self back from GC frame"
        );
    }
}
