//! Statement and expression emit: `emit_stmts`, `emit_stmt`, `emit_expr`,
//! `infer_valtype`, and the `binop_instr` dispatch table.

use crate::domain::ir::{IrBinOp, IrExpr, IrStmt, IrUnOp};
use std::collections::HashMap;
use super::{ir_to_val, ValTy, WasmCodegenError, WatGen};

impl WatGen {
    // ── Statement emission ────────────────────────────────────────────────────

    pub(crate) fn emit_stmts(
        &mut self,
        stmts: &[IrStmt],
        locals: &HashMap<String, ValTy>,
    ) -> Result<(), WasmCodegenError> {
        for stmt in stmts {
            self.emit_stmt(stmt, locals)?;
        }
        Ok(())
    }

    pub(crate) fn emit_stmt(
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
            // Match statements are a JS-backend feature; the WASM backend
            // does not yet support them. Skip with a no-op.
            IrStmt::Match { .. } => {}
        }
        Ok(())
    }

    // ── Expression emission ───────────────────────────────────────────────────
    //
    // Returns `true` if the expression leaves a value on the WASM stack.

    pub(crate) fn emit_expr(
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

    pub(crate) fn infer_valtype(&self, expr: &IrExpr, locals: &HashMap<String, ValTy>) -> ValTy {
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
}

// ── Binary operation instructions ─────────────────────────────────────────────

pub(crate) fn binop_instr(op: &IrBinOp, lhs_ty: ValTy) -> &'static str {
    match (op, lhs_ty) {
        (IrBinOp::Add, ValTy::I64) => "i64.add",
        (IrBinOp::Sub, ValTy::I64) => "i64.sub",
        (IrBinOp::Mul, ValTy::I64) => "i64.mul",
        (IrBinOp::Div, ValTy::I64) => "i64.div_s",
        (IrBinOp::Mod, ValTy::I64) => "i64.rem_s",
        (IrBinOp::Eq, ValTy::I64)  => "i64.eq",
        (IrBinOp::Ne, ValTy::I64)  => "i64.ne",
        (IrBinOp::Lt, ValTy::I64)  => "i64.lt_s",
        (IrBinOp::Gt, ValTy::I64)  => "i64.gt_s",
        (IrBinOp::Le, ValTy::I64)  => "i64.le_s",
        (IrBinOp::Ge, ValTy::I64)  => "i64.ge_s",
        (IrBinOp::Add, ValTy::I32) => "i32.add",
        (IrBinOp::Sub, ValTy::I32) => "i32.sub",
        (IrBinOp::Mul, ValTy::I32) => "i32.mul",
        (IrBinOp::Div, ValTy::I32) => "i32.div_s",
        (IrBinOp::Mod, ValTy::I32) => "i32.rem_s",
        (IrBinOp::Eq, ValTy::I32)  => "i32.eq",
        (IrBinOp::Ne, ValTy::I32)  => "i32.ne",
        (IrBinOp::Lt, ValTy::I32)  => "i32.lt_s",
        (IrBinOp::Gt, ValTy::I32)  => "i32.gt_s",
        (IrBinOp::Le, ValTy::I32)  => "i32.le_s",
        (IrBinOp::Ge, ValTy::I32)  => "i32.ge_s",
        (IrBinOp::And, _)          => "i32.and",
        (IrBinOp::Or, _)           => "i32.or",
    }
}
