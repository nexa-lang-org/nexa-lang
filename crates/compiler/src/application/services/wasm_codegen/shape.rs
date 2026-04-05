//! Shape map emit: `$gc_object_size` and `$gc_scan_object` WAT functions.

use super::{ClassShape, WatGen, HEADER_SIZE};

impl WatGen {
    // ── Shape map ────────────────────────────────────────────────────────────

    /// Emit `$gc_object_size` and `$gc_scan_object` using pre-collected shape data.
    ///
    /// `shape_data`: `(tag, class_name, total_size, [(field_offset, field_name)])`
    pub(crate) fn emit_shape_map(&mut self, shape_data: &[ClassShape]) {
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
}
