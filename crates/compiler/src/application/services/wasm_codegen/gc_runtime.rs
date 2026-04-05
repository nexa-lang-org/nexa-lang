//! GC runtime emit methods: globals, allocator, Cheney copying collector.

use super::{GcLayout, WatGen};

impl WatGen {
    // ── GC globals ────────────────────────────────────────────────────────────

    pub(crate) fn emit_gc_globals(&mut self, gc: &GcLayout) {
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

    pub(crate) fn emit_gc_alloc(&mut self) {
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

    // ── GC runtime functions ──────────────────────────────────────────────────

    pub(crate) fn emit_gc_runtime(&mut self, _gc: &GcLayout) {
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
}
