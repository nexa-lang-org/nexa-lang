//! Constructor and method emit: `build_frame`, `emit_frame_setup`,
//! `emit_constructor`, `emit_method`, `compile_class`.

use crate::domain::ir::{IrClass, IrMethod, IrType};
use std::collections::HashMap;
use super::{
    collect_let_locals, count_nodes_in_stmts, ir_to_val,
    ValTy, WasmCodegenError, WatGen, HEADER_SIZE,
};

impl WatGen {
    // ── Class compilation ─────────────────────────────────────────────────────

    pub(crate) fn compile_class(&mut self, cls: &IrClass) -> Result<(), WasmCodegenError> {
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

    // ── GC v2: shadow-stack frame map ─────────────────────────────────────────

    /// Build `gc_ptr_frame` for the current function.
    ///
    /// Frame layout (each slot = 4 bytes):
    ///   [0]     $self
    ///   [4…]    pointer-typed params (i32), in order
    ///   [N…]    i32 let-bound locals, in declaration order
    ///
    /// Always call this before emitting a constructor or method body.
    pub(crate) fn build_frame(
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
    pub(crate) fn emit_frame_setup(&mut self) {
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

    pub(crate) fn emit_constructor(&mut self, cls: &IrClass) -> Result<(), WasmCodegenError> {
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

    pub(crate) fn emit_method(
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
    pub(crate) fn frame_store_str(offset: u32, value_wat: &str) -> String {
        if offset == 0 {
            format!("(i32.store (local.get $__gc_frame) {value_wat})")
        } else {
            format!("(i32.store offset={offset} (local.get $__gc_frame) {value_wat})")
        }
    }
}
