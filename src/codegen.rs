use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::TargetMachine;
use inkwell::types::{PointerType, StructType};
use inkwell::values::{FunctionValue, IntValue, PointerValue, StructValue};
use inkwell::AddressSpace;

use crate::ast::*;
use crate::error::CompileError;
use crate::value::*;

pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    value_ty: StructType<'ctx>,
    closure_ty: StructType<'ctx>,
    array_ty: StructType<'ctx>,
    struct_hdr_ty: StructType<'ctx>,
    enum_hdr_ty: StructType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    globals: HashMap<String, PointerValue<'ctx>>,
    externs: HashMap<String, FunctionValue<'ctx>>,
    records: HashMap<String, RecordInfo<'ctx>>,
    variants: HashMap<String, VariantInfo<'ctx>>,
    imports: Vec<String>,
    std_imports: Vec<String>,
    fn_depth: u32,
    fn_counter: u32,
    cur_file: String,
}

/// Compile-time info for a declared `record`: a pointer to its static
/// descriptor global ({ i8* type_name, i64 nfields, [nfields x i8*]
/// field_names }) and the ordered field names, used for construction
/// arity-checking. Field *lookup* (get/set) is done by name at runtime
/// against the descriptor, so a struct value carries its own type.
#[derive(Clone)]
struct RecordInfo<'ctx> {
    descriptor: PointerValue<'ctx>,
    fields: Vec<String>,
}

/// Compile-time info for one variant of a declared `choice`. Keyed by
/// variant name (flat across all choices) so that variant construction and
/// `match` arms can resolve a bare variant name. The descriptor is a static
/// global shaped exactly like a record descriptor ({ i8* variant_name, i64
/// nfields, [nfields x i8*] field_names }), so enum print/GC reuse the struct
/// machinery. `variant_id` is the variant's index within its choice.
#[derive(Clone)]
struct VariantInfo<'ctx> {
    descriptor: PointerValue<'ctx>,
    variant_id: u64,
    fields: Vec<String>,
}

impl<'ctx> Codegen<'ctx> {
    pub fn new(ctx: &'ctx Context) -> Self {
        let module = ctx.create_module("verb");
        let builder = ctx.create_builder();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let value_ty = ctx.struct_type(&[ctx.i8_type().into(), ctx.i64_type().into()], false);
        let closure_ty =
            ctx.struct_type(&[ptr_ty.into(), ctx.i64_type().into(), ptr_ty.into()], false);
        let array_ty =
            ctx.struct_type(&[ctx.i64_type().into(), ctx.i64_type().into(), ptr_ty.into()], false);
        // Header shared by every struct/record heap block: { ptr descriptor,
        // i64 nfields }. The nfields inline `value_ty` fields follow at
        // byte offset 16 (see `struct_field_slot`).
        let struct_hdr_ty = ctx.struct_type(&[ptr_ty.into(), ctx.i64_type().into()], false);
        // Header shared by every enum heap block: { ptr descriptor, i64
        // variant_id, i64 nfields }. The nfields inline `value_ty` fields
        // follow at byte offset 24 (see `enum_field_slot`).
        let enum_hdr_ty =
            ctx.struct_type(&[ptr_ty.into(), ctx.i64_type().into(), ctx.i64_type().into()], false);
        let mut cg = Self {
            ctx, module, builder, value_ty, closure_ty, array_ty, struct_hdr_ty, enum_hdr_ty, ptr_ty,
            scopes: Vec::new(), globals: HashMap::new(), externs: HashMap::new(),
            records: HashMap::new(), variants: HashMap::new(),
            imports: Vec::new(), std_imports: Vec::new(), fn_depth: 0, fn_counter: 0,
            cur_file: String::new(),
        };
        cg.declare_libc();
        cg.declare_gc_globals();
        cg.build_alloc_fn();
        cg.build_type_name_fn();
        cg.build_print_value_fn();
        cg.build_print_fn();
        cg.build_truthy_fn();
        cg.build_arith_fns();
        cg.build_cmp_fns();
        cg.build_eq_fn();
        cg.build_concat_fn();
        cg.build_str_len_fn();
        cg.build_str_index_fn();
        cg.build_str_slice_fn();
        cg.build_str_split_fn();
        cg.build_neg_fn();
        cg.build_check_call_fn();
        cg.build_array_len_fn();
        cg.build_array_check_fn();
        // verb_retain_value/verb_release_value must be declared before
        // build_array_get_fn/build_array_set_fn, which now call_named
        // "verb_retain_value" while building their bodies (call_named
        // requires the callee to already exist in the module).
        cg.build_retain_value_fn();
        cg.build_release_value_fn();
        // verb_struct_get/set call verb_retain_value/verb_release_value, so
        // they must be built after those two exist in the module.
        cg.build_struct_get_fn();
        cg.build_struct_set_fn();
        cg.build_array_get_fn();
        cg.build_array_set_fn();
        cg.build_array_push_fn();
        cg.build_array_pop_fn();
        cg.build_retain_cell_fn();
        cg.build_release_cell_fn();
        // Built-in `Result` choice: `Ok(value)` and `Err(kind, msg)` are
        // predeclared variants, so result-style error handling reuses the
        // whole enum machinery -- construction (`Ok(x)`/`Err(k, m)` in
        // gen_call), `match`/`when`, printing, and GC -- with no new tag.
        // `Err`'s variant_id backs the `is_err`/`err_kind`/`err_msg`
        // builtins and the std-io nil->Err failure lift (see gen_std_io_call).
        cg.gen_choice("Result", &[
            ("Ok".to_string(), vec!["value".to_string()]),
            ("Err".to_string(), vec!["kind".to_string(), "msg".to_string()]),
        ]);
        cg
    }

    pub fn module(&self) -> &Module<'ctx> { &self.module }

    /// Runs the LLVM new-pass-manager optimization pipeline `default<O{level}>`
    /// over the whole module, targeting `tm`. `level` 0 is a no-op (the
    /// unoptimized module is emitted verbatim). Levels are clamped to 3.
    ///
    /// DCE safety: codegen's GC `verb_release_value` (and `verb_retain_value`)
    /// calls target *external* declarations with no `readnone`/`willreturn`
    /// attributes, so LLVM must assume they have observable side effects and
    /// never eliminates them — even aggressive DCE at `-O2`/`-O3` keeps every
    /// release, so the `assert_no_leaks` fixtures stay balanced.
    pub fn optimize(&self, tm: &TargetMachine, level: u8) -> Result<(), String> {
        if level == 0 {
            return Ok(());
        }
        let level = level.min(3);
        self.module
            .run_passes(&format!("default<O{level}>"), tm, PassBuilderOptions::create())
            .map_err(|e| e.to_string())
    }

    fn declare_libc(&self) {
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let pt = self.ptr_ty;
        self.module.add_function("printf", i32t.fn_type(&[pt.into()], true), None);
        self.module.add_function("malloc", pt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("exit", self.ctx.void_type().fn_type(&[i32t.into()], false), None);
        self.module.add_function("strlen", i64t.fn_type(&[pt.into()], false), None);
        self.module.add_function("strcpy", pt.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("strcat", pt.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("strstr", pt.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("memcpy", pt.fn_type(&[pt.into(), pt.into(), i64t.into()], false), None);
        self.module.add_function("free", self.ctx.void_type().fn_type(&[pt.into()], false), None);
        self.module.add_function("getenv", pt.fn_type(&[pt.into()], false), None);
        self.module.add_function(
            "verb_map_destroy_contents", self.ctx.void_type().fn_type(&[pt.into()], false), None);
    }

    fn declare_gc_globals(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.add_global(i64t, None, "verb_gc_live");
        g.set_initializer(&i64t.const_zero());
    }

    fn inc_live_counter(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let cur = self.builder.build_load(i64t, g, "gc_live").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i64t.const_int(1, false), "gc_live1").unwrap();
        self.builder.build_store(g, next).unwrap();
    }

    /// Given a payload pointer (what a `VerbValue` or a cell already
    /// points at), returns a pointer to its 8-byte refcount header,
    /// living immediately before it. Valid for every string, closure,
    /// array, map, and cell pointer Verb ever produces.
    fn header_ptr(&self, payload: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), payload, &[i64t.const_int((-8i64) as u64, true)], "hdr")
        }.unwrap()
    }

    /// Pointer to field slot `idx` (a `value_ty`) inside a struct/record
    /// heap block whose payload pointer is `sp`. Fields live inline after
    /// the 16-byte `struct_hdr_ty` header, so slot `idx` is at
    /// `sp + 16 + idx*16`. `idx` may be a compile-time constant (construction)
    /// or a runtime value (GC cascade, field lookup).
    fn struct_field_slot(&self, sp: PointerValue<'ctx>, idx: IntValue<'ctx>) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let base = unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), sp, &[i64t.const_int(16, false)], "fbase")
        }.unwrap();
        unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, base, &[idx], "fslot")
        }.unwrap()
    }

    /// Pointer to field slot `idx` (a `value_ty`) inside an enum heap block
    /// whose payload pointer is `sp`. Fields live inline after the 24-byte
    /// `enum_hdr_ty` header (descriptor + variant_id + nfields), so slot
    /// `idx` is at `sp + 24 + idx*16`.
    fn enum_field_slot(&self, sp: PointerValue<'ctx>, idx: IntValue<'ctx>) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let base = unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), sp, &[i64t.const_int(24, false)], "efbase")
        }.unwrap();
        unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, base, &[idx], "efslot")
        }.unwrap()
    }

    fn dec_live_counter(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let cur = self.builder.build_load(i64t, g, "gc_live").unwrap().into_int_value();
        let next = self.builder.build_int_sub(cur, i64t.const_int(1, false), "gc_live1").unwrap();
        self.builder.build_store(g, next).unwrap();
    }

    // ----- value helpers -----

    fn make_val(&self, tag: u64, payload: IntValue<'ctx>) -> StructValue<'ctx> {
        let t = self.ctx.i8_type().const_int(tag, false);
        let v = self.value_ty.get_undef();
        let v = self.builder.build_insert_value(v, t, 0, "vt").unwrap().into_struct_value();
        self.builder.build_insert_value(v, payload, 1, "vp").unwrap().into_struct_value()
    }

    fn nil_val(&self) -> StructValue<'ctx> {
        self.make_val(TAG_NIL, self.ctx.i64_type().const_zero())
    }

    fn tag_of(&self, v: StructValue<'ctx>) -> IntValue<'ctx> {
        self.builder.build_extract_value(v, 0, "tag").unwrap().into_int_value()
    }

    fn payload_of(&self, v: StructValue<'ctx>) -> IntValue<'ctx> {
        self.builder.build_extract_value(v, 1, "pay").unwrap().into_int_value()
    }

    fn cstr(&self, s: &str) -> PointerValue<'ctx> {
        self.builder.build_global_string_ptr(s, "str").unwrap().as_pointer_value()
    }

    /// Builds a global for a Verb string *literal*: an i64 sentinel header
    /// immediately followed by the NUL-terminated bytes, laid out
    /// identically to a heap `verb_alloc` block (header at payload-8) so
    /// `verb_retain_value`/`verb_release_value` (Task 2) can treat every
    /// string pointer the same way. Returns a pointer to the byte data
    /// (not the header) -- exactly what `Expr::Str` needs.
    fn static_string_ptr(&self, s: &str) -> PointerValue<'ctx> {
        let i8t = self.ctx.i8_type();
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let mut data: Vec<u8> = s.as_bytes().to_vec();
        data.push(0);
        let arr_ty = i8t.array_type(data.len() as u32);
        let struct_ty = self.ctx.struct_type(&[i64t.into(), arr_ty.into()], false);
        let hdr = i64t.const_int(GC_STATIC_SENTINEL as u64, true);
        let arr_vals: Vec<_> = data.iter().map(|b| i8t.const_int(*b as u64, false)).collect();
        let arr = i8t.const_array(&arr_vals);
        let init = struct_ty.const_named_struct(&[hdr.into(), arr.into()]);
        let g = self.module.add_global(struct_ty, None, "verb.strlit");
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(inkwell::module::Linkage::Private);
        g.set_unnamed_addr(true);
        unsafe {
            self.builder.build_in_bounds_gep(
                struct_ty, g.as_pointer_value(),
                &[i32t.const_zero(), i32t.const_int(1, false), i32t.const_zero()],
                "strdata",
            )
        }.unwrap()
    }

    fn call_named(&self, name: &str, args: &[inkwell::values::BasicMetadataValueEnum<'ctx>])
        -> Option<inkwell::values::BasicValueEnum<'ctx>>
    {
        let f = self.module.get_function(name).unwrap();
        self.builder.build_call(f, args, "").unwrap().try_as_basic_value().basic()
    }

    /// Abort with source location and optional printf extras (e.g. %s type names).
    fn abort_at(&self, line: IntValue<'ctx>, col: IntValue<'ctx>, fmt_tail: &str,
                extras: &[inkwell::values::BasicMetadataValueEnum<'ctx>])
    {
        let s = self.cstr(&format!("runtime error [%d:%d]: {fmt_tail}\n"));
        let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![s.into(), line.into(), col.into()];
        args.extend_from_slice(extras);
        self.call_named("printf", &args);
        self.call_named("exit", &[self.ctx.i32_type().const_int(1, false).into()]);
        self.builder.build_unreachable().unwrap();
    }

    /// Abort with a plain message and no source location -- used where no
    /// line/col is available (e.g. inside `verb_alloc`). Same printf + exit(1)
    /// + unreachable trailer as `abort_at`, but the caller supplies the full
    /// (already newline-terminated) message.
    fn abort_msg(&self, msg: &str) {
        let s = self.cstr(msg);
        self.call_named("printf", &[s.into()]);
        self.call_named("exit", &[self.ctx.i32_type().const_int(1, false).into()]);
        self.builder.build_unreachable().unwrap();
    }

    /// Get (or lazily declare) an LLVM overflow-checked-arithmetic intrinsic,
    /// e.g. `llvm.umul.with.overflow.i64`. All of these have the shape
    /// `{i64, i1} (i64, i64)` -- the i1 is the overflow flag.
    fn overflow_intrinsic(&self, name: &str) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let i64t = self.ctx.i64_type();
        let i1t = self.ctx.bool_type();
        let ret_ty = self.ctx.struct_type(&[i64t.into(), i1t.into()], false);
        let fnty = ret_ty.fn_type(&[i64t.into(), i64t.into()], false);
        self.module.add_function(name, fnty, None)
    }

    /// Emit a call to an overflow-checked arithmetic intrinsic, branch on the
    /// overflow flag: on overflow abort (with a source location via `abort_at`
    /// if `loc` is `Some`, else a plain `abort_msg`), otherwise fall through
    /// and return the computed result. Positions the builder on the
    /// non-overflow continuation block before returning.
    fn checked_binop(&self, intrinsic: &str, a: IntValue<'ctx>, b: IntValue<'ctx>,
                     loc: Option<(IntValue<'ctx>, IntValue<'ctx>)>) -> IntValue<'ctx>
    {
        let f = self.overflow_intrinsic(intrinsic);
        let agg = self.builder.build_call(f, &[a.into(), b.into()], "ovf")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
        let result = self.builder.build_extract_value(agg, 0, "ovf.res").unwrap().into_int_value();
        let ovf = self.builder.build_extract_value(agg, 1, "ovf.bit").unwrap().into_int_value();
        let cur_fn = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let abort_bb = self.ctx.append_basic_block(cur_fn, "ovf.abort");
        let cont_bb = self.ctx.append_basic_block(cur_fn, "ovf.cont");
        self.builder.build_conditional_branch(ovf, abort_bb, cont_bb).unwrap();
        self.builder.position_at_end(abort_bb);
        match loc {
            Some((line, col)) => self.abort_at(line, col, "integer overflow", &[]),
            None => self.abort_msg("runtime error: integer overflow\n"),
        }
        self.builder.position_at_end(cont_bb);
        result
    }

    /// Checked `a + b` (aborts on overflow). `signed` selects the signed
    /// (language int) vs unsigned (size computation) intrinsic.
    fn checked_add_or_abort(&self, a: IntValue<'ctx>, b: IntValue<'ctx>,
                            loc: Option<(IntValue<'ctx>, IntValue<'ctx>)>, signed: bool)
        -> IntValue<'ctx>
    {
        let name = if signed { "llvm.sadd.with.overflow.i64" } else { "llvm.uadd.with.overflow.i64" };
        self.checked_binop(name, a, b, loc)
    }

    /// Checked `a * b` (aborts on overflow). `signed` selects the signed
    /// (language int) vs unsigned (size computation) intrinsic.
    fn checked_mul_or_abort(&self, a: IntValue<'ctx>, b: IntValue<'ctx>,
                            loc: Option<(IntValue<'ctx>, IntValue<'ctx>)>, signed: bool)
        -> IntValue<'ctx>
    {
        let name = if signed { "llvm.smul.with.overflow.i64" } else { "llvm.umul.with.overflow.i64" };
        self.checked_binop(name, a, b, loc)
    }

    /// Checked signed `a - b` (aborts on overflow). Language arithmetic only;
    /// there is no unsigned subtraction site.
    fn checked_sub_or_abort(&self, a: IntValue<'ctx>, b: IntValue<'ctx>,
                            loc: Option<(IntValue<'ctx>, IntValue<'ctx>)>) -> IntValue<'ctx>
    {
        self.checked_binop("llvm.ssub.with.overflow.i64", a, b, loc)
    }

    /// Runtime type name of a tag, as a printf %s argument.
    fn type_name(&self, tag: IntValue<'ctx>) -> inkwell::values::BasicMetadataValueEnum<'ctx> {
        self.call_named("verb_type_name", &[tag.into()]).unwrap().into()
    }

    /// verb_type_name(i8 tag) -> ptr to static name string.
    fn build_type_name_fn(&self) {
        let f = self.module.add_function(
            "verb_type_name", self.ptr_ty.fn_type(&[self.ctx.i8_type().into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let tag = f.get_nth_param(0).unwrap().into_int_value();
        let i8t = self.ctx.i8_type();
        let default_bb = self.ctx.append_basic_block(f, "default");
        let mut cases = Vec::new();
        for (t, name) in [(TAG_NIL, "nil"), (TAG_BOOL, "bool"), (TAG_INT, "int"),
                          (TAG_FLOAT, "float"), (TAG_STR, "string"), (TAG_CLOSURE, "fn"),
                          (TAG_ARRAY, "array"), (TAG_MAP, "map"), (TAG_STRUCT, "struct"),
                          (TAG_ENUM, "enum")] {
            let bb = self.ctx.append_basic_block(f, name);
            self.builder.position_at_end(bb);
            let s = self.cstr(name);
            self.builder.build_return(Some(&s)).unwrap();
            cases.push((i8t.const_int(t, false), bb));
        }
        self.builder.position_at_end(default_bb);
        let s = self.cstr("value");
        self.builder.build_return(Some(&s)).unwrap();
        self.builder.position_at_end(entry);
        self.builder.build_switch(tag, default_bb, &cases).unwrap();
    }

    /// Runtime helper: verb_alloc(i64 n) -> ptr. Wraps `malloc` with an
    /// 8-byte refcount header (initialized to 1) prefixed to every heap
    /// block Verb owns; the returned pointer is the payload -- the header
    /// lives at payload-8. String literals get the same header shape
    /// baked into their LLVM global (see `static_string_ptr`) so
    /// retain/release never need to know statically whether a given
    /// string pointer is heap or static.
    fn build_alloc_fn(&self) {
        let i64t = self.ctx.i64_type();
        let fnty = self.ptr_ty.fn_type(&[i64t.into()], false);
        let f = self.module.add_function("verb_alloc", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let n = f.get_nth_param(0).unwrap().into_int_value();
        // Header add is a size computation: a pathological `n` near i64::MAX
        // must not silently wrap the +8 and hand malloc a tiny size.
        let total = self.checked_add_or_abort(n, i64t.const_int(8, false), None, false);
        let raw = self.call_named("malloc", &[total.into()]).unwrap().into_pointer_value();
        // OOM: on malloc failure the pointer is null; storing the refcount
        // below would null-deref (segfault). Split and abort cleanly instead.
        let is_null = self.builder.build_is_null(raw, "malloc_null").unwrap();
        let oom_bb = self.ctx.append_basic_block(f, "oom");
        let ok_bb = self.ctx.append_basic_block(f, "alloc.ok");
        self.builder.build_conditional_branch(is_null, oom_bb, ok_bb).unwrap();

        self.builder.position_at_end(oom_bb);
        self.abort_msg("out of memory\n");

        // Success path: store header, compute payload, count the live block.
        // inc_live_counter MUST stay here (never on the OOM path).
        self.builder.position_at_end(ok_bb);
        self.builder.build_store(raw, i64t.const_int(1, false)).unwrap();
        let payload = unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), raw, &[i64t.const_int(8, false)], "payload")
        }.unwrap();
        self.inc_live_counter();
        self.builder.build_return(Some(&payload)).unwrap();
    }

    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("verb_alloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }

    fn release_scope(&self, scope: &HashMap<String, PointerValue<'ctx>>) {
        for cell in scope.values() {
            self.call_named("verb_release_cell", &[(*cell).into()]);
        }
    }

    /// Releases every cell in every currently-open scope (this function's
    /// own scope stack -- already isolated per-function via the
    /// `saved_scopes` swap in `Stmt::Fn`), innermost first. Read-only over
    /// `self.scopes`: never pops. Must run immediately before *every*
    /// path that can leave a function or the top-level program -- an
    /// explicit `return`, or an implicit end-of-body/end-of-program
    /// return -- since Step 2's scope-pop cleanup only fires on normal
    /// block fall-through and is skipped once a block is already
    /// terminated.
    fn release_all_open_scopes(&self) {
        for scope in self.scopes.iter().rev() {
            self.release_scope(scope);
        }
    }

    /// If `VERB_GC_DEBUG` is set in the environment, prints
    /// `verb_gc_live=<n>` to stdout, where `<n>` is the number of
    /// outstanding `verb_alloc` blocks (strings/closures/arrays/maps/cells)
    /// at program exit. Purely a test/debugging hook -- silent otherwise,
    /// and never affects a program's own output.
    fn emit_gc_debug_dump(&self, main: FunctionValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let env_name = self.cstr("VERB_GC_DEBUG");
        let flag = self.call_named("getenv", &[env_name.into()]).unwrap().into_pointer_value();
        let flag_int = self.builder.build_ptr_to_int(flag, i64t, "flagi").unwrap();
        let is_set = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, flag_int, i64t.const_zero(), "gc_debug").unwrap();
        let dbg_bb = self.ctx.append_basic_block(main, "gc.debug");
        let cont_bb = self.ctx.append_basic_block(main, "gc.cont");
        self.builder.build_conditional_branch(is_set, dbg_bb, cont_bb).unwrap();

        self.builder.position_at_end(dbg_bb);
        let live_ptr = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let live = self.builder.build_load(i64t, live_ptr, "live").unwrap();
        let fmt = self.cstr("verb_gc_live=%lld\n");
        self.call_named("printf", &[fmt.into(), live.into()]);
        self.builder.build_unconditional_branch(cont_bb).unwrap();

        self.builder.position_at_end(cont_bb);
    }

    /// Like `malloc_bytes`, but the size is a runtime value (used when an
    /// array buffer's size depends on its element count, not a fixed layout).
    fn malloc_bytes_dyn(&self, n: IntValue<'ctx>) -> PointerValue<'ctx> {
        self.call_named("verb_alloc", &[n.into()]).unwrap().into_pointer_value()
    }

    // ----- generated runtime helper: verb_print_value(value) — no trailing newline -----

    fn build_print_value_fn(&self) {
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_print_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let tag = self.tag_of(v);
        let pay = self.payload_of(v);

        let nil_bb = self.ctx.append_basic_block(f, "nil");
        let bool_bb = self.ctx.append_basic_block(f, "bool");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let float_bb = self.ctx.append_basic_block(f, "float");
        let str_bb = self.ctx.append_basic_block(f, "string");
        let clos_bb = self.ctx.append_basic_block(f, "closure");
        let arr_bb = self.ctx.append_basic_block(f, "array");
        let map_bb = self.ctx.append_basic_block(f, "map");
        let struct_bb = self.ctx.append_basic_block(f, "struct");
        let enum_bb = self.ctx.append_basic_block(f, "enum");
        let done = self.ctx.append_basic_block(f, "done");

        let i8t = self.ctx.i8_type();
        self.builder.build_switch(tag, done, &[
            (i8t.const_int(TAG_NIL, false), nil_bb),
            (i8t.const_int(TAG_BOOL, false), bool_bb),
            (i8t.const_int(TAG_INT, false), int_bb),
            (i8t.const_int(TAG_FLOAT, false), float_bb),
            (i8t.const_int(TAG_STR, false), str_bb),
            (i8t.const_int(TAG_CLOSURE, false), clos_bb),
            (i8t.const_int(TAG_ARRAY, false), arr_bb),
            (i8t.const_int(TAG_MAP, false), map_bb),
            (i8t.const_int(TAG_STRUCT, false), struct_bb),
            (i8t.const_int(TAG_ENUM, false), enum_bb),
        ]).unwrap();

        self.builder.position_at_end(nil_bb);
        self.call_named("printf", &[self.cstr("nil").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(bool_bb);
        let is_true = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, pay, self.ctx.i64_type().const_zero(), "istrue").unwrap();
        let ts = self.cstr("true");
        let fs = self.cstr("false");
        let sel = self.builder.build_select(is_true, ts, fs, "boolstr").unwrap();
        self.call_named("printf", &[sel.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(int_bb);
        self.call_named("printf", &[self.cstr("%lld").into(), pay.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(float_bb);
        let fv = self.builder.build_bit_cast(pay, self.ctx.f64_type(), "f").unwrap();
        self.call_named("printf", &[self.cstr("%g").into(), fv.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(pay, self.ptr_ty, "sptr").unwrap();
        self.call_named("printf", &[self.cstr("%s").into(), sp.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(clos_bb);
        self.call_named("printf", &[self.cstr("<fn>").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(arr_bb);
        let hdr = self.builder.build_int_to_ptr(pay, self.ptr_ty, "hdr").unwrap();
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        let len = self.builder.build_load(self.ctx.i64_type(), lenp, "len").unwrap().into_int_value();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        self.call_named("printf", &[self.cstr("[").into()]);

        let idxp = self.entry_alloca(self.ctx.i64_type().into(), "pidx");
        self.builder.build_store(idxp, self.ctx.i64_type().const_zero()).unwrap();
        let cond_bb = self.ctx.append_basic_block(f, "print.cond");
        let body_bb = self.ctx.append_basic_block(f, "print.body");
        let sep_bb = self.ctx.append_basic_block(f, "print.sep");
        let elem_bb = self.ctx.append_basic_block(f, "print.elem");
        let end_bb = self.ctx.append_basic_block(f, "print.end");
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(cond_bb);
        let i = self.builder.build_load(self.ctx.i64_type(), idxp, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(inkwell::IntPredicate::SLT, i, len, "more").unwrap();
        self.builder.build_conditional_branch(more, body_bb, end_bb).unwrap();

        self.builder.position_at_end(body_bb);
        let is_first = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, i, self.ctx.i64_type().const_zero(), "isfirst").unwrap();
        self.builder.build_conditional_branch(is_first, elem_bb, sep_bb).unwrap();

        self.builder.position_at_end(sep_bb);
        self.call_named("printf", &[self.cstr(", ").into()]);
        self.builder.build_unconditional_branch(elem_bb).unwrap();

        self.builder.position_at_end(elem_bb);
        let slot = unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot")
        }.unwrap();
        let elemv = self.builder.build_load(self.value_ty, slot, "elemv").unwrap().into_struct_value();
        self.call_named("verb_print_value", &[elemv.into()]);
        let next = self.builder.build_int_add(i, self.ctx.i64_type().const_int(1, false), "next").unwrap();
        self.builder.build_store(idxp, next).unwrap();
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(end_bb);
        self.call_named("printf", &[self.cstr("]").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(map_bb);
        self.call_named("printf", &[self.cstr("<map>\n").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        // ----- struct/record: Point{x: <v>, y: <v>} (generic via descriptor) -----
        self.builder.position_at_end(struct_bb);
        let i64t = self.ctx.i64_type();
        let ssp = self.builder.build_int_to_ptr(pay, self.ptr_ty, "ssp").unwrap();
        let sdescp = self.builder.build_struct_gep(self.struct_hdr_ty, ssp, 0, "sdescp").unwrap();
        let sdesc = self.builder.build_load(self.ptr_ty, sdescp, "sdesc").unwrap().into_pointer_value();
        // descriptor field 0 = type_name (i8*)
        let tnamep = self.builder.build_struct_gep(self.struct_hdr_ty, sdesc, 0, "tnamep").unwrap();
        let tname = self.builder.build_load(self.ptr_ty, tnamep, "tname").unwrap();
        self.call_named("printf", &[self.cstr("%s{").into(), tname.into()]);
        let snfp = self.builder.build_struct_gep(self.struct_hdr_ty, ssp, 1, "snfp").unwrap();
        let snf = self.builder.build_load(i64t, snfp, "snf").unwrap().into_int_value();

        let sidxp = self.entry_alloca(i64t.into(), "spidx");
        self.builder.build_store(sidxp, i64t.const_zero()).unwrap();
        let scond_bb = self.ctx.append_basic_block(f, "sp.cond");
        let sbody_bb = self.ctx.append_basic_block(f, "sp.body");
        let ssep_bb = self.ctx.append_basic_block(f, "sp.sep");
        let sfield_bb = self.ctx.append_basic_block(f, "sp.field");
        let send_bb = self.ctx.append_basic_block(f, "sp.end");
        self.builder.build_unconditional_branch(scond_bb).unwrap();

        self.builder.position_at_end(scond_bb);
        let si = self.builder.build_load(i64t, sidxp, "si").unwrap().into_int_value();
        let smore = self.builder.build_int_compare(inkwell::IntPredicate::SLT, si, snf, "smore").unwrap();
        self.builder.build_conditional_branch(smore, sbody_bb, send_bb).unwrap();

        self.builder.position_at_end(sbody_bb);
        let sfirst = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, si, i64t.const_zero(), "sfirst").unwrap();
        self.builder.build_conditional_branch(sfirst, sfield_bb, ssep_bb).unwrap();

        self.builder.position_at_end(ssep_bb);
        self.call_named("printf", &[self.cstr(", ").into()]);
        self.builder.build_unconditional_branch(sfield_bb).unwrap();

        self.builder.position_at_end(sfield_bb);
        // field name i: descriptor's names array begins at desc + 16, each ptr-sized.
        let noff = self.builder.build_int_mul(si, i64t.const_int(8, false), "noff").unwrap();
        let noff = self.builder.build_int_add(noff, i64t.const_int(16, false), "noff16").unwrap();
        let namep_addr = unsafe {
            self.builder.build_in_bounds_gep(self.ctx.i8_type(), sdesc, &[noff], "namep_addr")
        }.unwrap();
        let fname = self.builder.build_load(self.ptr_ty, namep_addr, "fname").unwrap();
        self.call_named("printf", &[self.cstr("%s: ").into(), fname.into()]);
        let sslot = self.struct_field_slot(ssp, si);
        let sfv = self.builder.build_load(self.value_ty, sslot, "sfv").unwrap().into_struct_value();
        self.call_named("verb_print_value", &[sfv.into()]);
        let snext = self.builder.build_int_add(si, i64t.const_int(1, false), "snext").unwrap();
        self.builder.build_store(sidxp, snext).unwrap();
        self.builder.build_unconditional_branch(scond_bb).unwrap();

        self.builder.position_at_end(send_bb);
        self.call_named("printf", &[self.cstr("}").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        // ----- enum/variant: Circle(<v>, <v>) (name from descriptor, then a
        // parenthesized, comma-separated field list; bare name if no fields) -----
        self.builder.position_at_end(enum_bb);
        let esp = self.builder.build_int_to_ptr(pay, self.ptr_ty, "esp").unwrap();
        let edescp = self.builder.build_struct_gep(self.enum_hdr_ty, esp, 0, "edescp").unwrap();
        let edesc = self.builder.build_load(self.ptr_ty, edescp, "edesc").unwrap().into_pointer_value();
        // descriptor field 0 = variant_name (shaped exactly like a record descriptor)
        let enamep = self.builder.build_struct_gep(self.struct_hdr_ty, edesc, 0, "enamep").unwrap();
        let ename = self.builder.build_load(self.ptr_ty, enamep, "ename").unwrap();
        self.call_named("printf", &[self.cstr("%s").into(), ename.into()]);
        let enfp = self.builder.build_struct_gep(self.enum_hdr_ty, esp, 2, "enfp").unwrap();
        let enf = self.builder.build_load(i64t, enfp, "enf").unwrap().into_int_value();
        let ehas = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, enf, i64t.const_zero(), "ehas").unwrap();
        let eopen_bb = self.ctx.append_basic_block(f, "ep.open");
        let econd_bb = self.ctx.append_basic_block(f, "ep.cond");
        let ebody_bb = self.ctx.append_basic_block(f, "ep.body");
        let esep_bb = self.ctx.append_basic_block(f, "ep.sep");
        let efield_bb = self.ctx.append_basic_block(f, "ep.field");
        let eclose_bb = self.ctx.append_basic_block(f, "ep.close");
        self.builder.build_conditional_branch(ehas, eopen_bb, done).unwrap();

        self.builder.position_at_end(eopen_bb);
        self.call_named("printf", &[self.cstr("(").into()]);
        let eidxp = self.entry_alloca(i64t.into(), "epidx");
        self.builder.build_store(eidxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(econd_bb).unwrap();

        self.builder.position_at_end(econd_bb);
        let ei = self.builder.build_load(i64t, eidxp, "ei").unwrap().into_int_value();
        let emore = self.builder.build_int_compare(inkwell::IntPredicate::SLT, ei, enf, "emore").unwrap();
        self.builder.build_conditional_branch(emore, ebody_bb, eclose_bb).unwrap();

        self.builder.position_at_end(ebody_bb);
        let efirst = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, ei, i64t.const_zero(), "efirst").unwrap();
        self.builder.build_conditional_branch(efirst, efield_bb, esep_bb).unwrap();

        self.builder.position_at_end(esep_bb);
        self.call_named("printf", &[self.cstr(", ").into()]);
        self.builder.build_unconditional_branch(efield_bb).unwrap();

        self.builder.position_at_end(efield_bb);
        let eslot = self.enum_field_slot(esp, ei);
        let efv = self.builder.build_load(self.value_ty, eslot, "efv").unwrap().into_struct_value();
        self.call_named("verb_print_value", &[efv.into()]);
        let enext = self.builder.build_int_add(ei, i64t.const_int(1, false), "enext").unwrap();
        self.builder.build_store(eidxp, enext).unwrap();
        self.builder.build_unconditional_branch(econd_bb).unwrap();

        self.builder.position_at_end(eclose_bb);
        self.call_named("printf", &[self.cstr(")").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(done);
        self.builder.build_return(None).unwrap();
    }

    // ----- generated runtime helper: verb_print(value) — adds trailing newline -----

    fn build_print_fn(&self) {
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_print", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        self.call_named("verb_print_value", &[v.into()]);
        self.call_named("printf", &[self.cstr("\n").into()]);
        self.builder.build_return(None).unwrap();
    }

    // ----- generated runtime helpers: operators -----

    /// truthy = tag != NIL && (tag != BOOL || payload != 0)   (branch-free)
    fn build_truthy_fn(&self) {
        let f = self.module.add_function(
            "verb_truthy", self.ctx.bool_type().fn_type(&[self.value_ty.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let tag = self.tag_of(v);
        let pay = self.payload_of(v);
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let not_nil = self.builder.build_int_compare(NE, tag, i8t.const_int(TAG_NIL, false), "nn").unwrap();
        let not_bool = self.builder.build_int_compare(NE, tag, i8t.const_int(TAG_BOOL, false), "nb").unwrap();
        let pay_nz = self.builder.build_int_compare(NE, pay, self.ctx.i64_type().const_zero(), "pnz").unwrap();
        let bool_ok = self.builder.build_or(not_bool, pay_nz, "bok").unwrap();
        let r = self.builder.build_and(not_nil, bool_ok, "truthy").unwrap();
        self.builder.build_return(Some(&r)).unwrap();
    }

    fn is_numeric(&self, tag: IntValue<'ctx>) -> IntValue<'ctx> {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let is_i = self.builder.build_int_compare(EQ, tag, i8t.const_int(TAG_INT, false), "isi").unwrap();
        let is_f = self.builder.build_int_compare(EQ, tag, i8t.const_int(TAG_FLOAT, false), "isf").unwrap();
        self.builder.build_or(is_i, is_f, "isnum").unwrap()
    }

    /// payload -> f64: int payload sitofp, float payload bitcast (select, both computed)
    fn to_f64(&self, tag: IntValue<'ctx>, pay: IntValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        use inkwell::IntPredicate::*;
        let is_int = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_INT, false), "isint").unwrap();
        let from_int = self.builder.build_signed_int_to_float(pay, self.ctx.f64_type(), "si").unwrap();
        let from_bits = self.builder.build_bit_cast(pay, self.ctx.f64_type(), "fb").unwrap().into_float_value();
        self.builder.build_select(is_int, from_int, from_bits, "f").unwrap().into_float_value()
    }

    fn f64_val(&self, f: inkwell::values::FloatValue<'ctx>) -> StructValue<'ctx> {
        let bits = self.builder.build_bit_cast(f, self.ctx.i64_type(), "bits").unwrap().into_int_value();
        self.make_val(TAG_FLOAT, bits)
    }

    fn bool_val(&self, b: IntValue<'ctx>) -> StructValue<'ctx> {
        let z = self.builder.build_int_z_extend(b, self.ctx.i64_type(), "bz").unwrap();
        self.make_val(TAG_BOOL, z)
    }

    fn build_arith_fns(&self) {
        for (name, kw, op) in [("verb_add", "add", BinOp::Add), ("verb_sub", "sub", BinOp::Sub),
                               ("verb_mul", "times", BinOp::Mul), ("verb_div", "div", BinOp::Div),
                               ("verb_mod", "mod", BinOp::Mod)] {
            self.build_arith_fn(name, kw, op);
        }
    }

    /// Helper signature: (value, value, i32 line, i32 col) -> value
    fn build_arith_fn(&self, name: &str, kw: &str, op: BinOp) {
        use inkwell::IntPredicate::*;
        let i32t = self.ctx.i32_type();
        let fnty = self.value_ty.fn_type(
            &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false);
        let f = self.module.add_function(name, fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chknum");
        let flt_bb = self.ctx.append_basic_block(f, "float");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let i8t = self.ctx.i8_type();
        let ai = self.builder.build_int_compare(EQ, ta, i8t.const_int(TAG_INT, false), "ai").unwrap();
        let bi = self.builder.build_int_compare(EQ, tb, i8t.const_int(TAG_INT, false), "bi").unwrap();
        let both_int = self.builder.build_and(ai, bi, "bothint").unwrap();
        self.builder.build_conditional_branch(both_int, int_bb, chk_bb).unwrap();

        // integer path
        self.builder.position_at_end(int_bb);
        if matches!(op, BinOp::Div | BinOp::Mod) {
            let zero_bb = self.ctx.append_basic_block(f, "izero");
            let go_bb = self.ctx.append_basic_block(f, "igo");
            let nz = self.builder.build_int_compare(
                NE, pb, self.ctx.i64_type().const_zero(), "nz").unwrap();
            self.builder.build_conditional_branch(nz, go_bb, zero_bb).unwrap();
            self.builder.position_at_end(zero_bb);
            self.abort_at(line, col, "division by zero", &[]);
            self.builder.position_at_end(go_bb);
        }
        let loc = Some((line, col));
        let ir = match op {
            BinOp::Add => self.checked_add_or_abort(pa, pb, loc, true),
            BinOp::Sub => self.checked_sub_or_abort(pa, pb, loc),
            BinOp::Mul => self.checked_mul_or_abort(pa, pb, loc, true),
            BinOp::Div => self.builder.build_int_signed_div(pa, pb, "r").unwrap(),
            BinOp::Mod => self.builder.build_int_signed_rem(pa, pb, "r").unwrap(),
            _ => unreachable!(),
        };
        let rv = self.make_val(TAG_INT, ir);
        self.builder.build_return(Some(&rv)).unwrap();

        // numeric check
        self.builder.position_at_end(chk_bb);
        let an = self.is_numeric(ta);
        let bn = self.is_numeric(tb);
        let both_num = self.builder.build_and(an, bn, "bothnum").unwrap();
        self.builder.build_conditional_branch(both_num, flt_bb, err_bb).unwrap();

        // float path (mixed promotes)
        self.builder.position_at_end(flt_bb);
        let fa = self.to_f64(ta, pa);
        let fb = self.to_f64(tb, pb);
        if matches!(op, BinOp::Div | BinOp::Mod) {
            let zero_bb = self.ctx.append_basic_block(f, "fzero");
            let go_bb = self.ctx.append_basic_block(f, "fgo");
            let nz = self.builder.build_float_compare(
                inkwell::FloatPredicate::ONE, fb, self.ctx.f64_type().const_zero(), "fnz").unwrap();
            self.builder.build_conditional_branch(nz, go_bb, zero_bb).unwrap();
            self.builder.position_at_end(zero_bb);
            self.abort_at(line, col, "division by zero", &[]);
            self.builder.position_at_end(go_bb);
        }
        let fr = match op {
            BinOp::Add => self.builder.build_float_add(fa, fb, "fr").unwrap(),
            BinOp::Sub => self.builder.build_float_sub(fa, fb, "fr").unwrap(),
            BinOp::Mul => self.builder.build_float_mul(fa, fb, "fr").unwrap(),
            BinOp::Div => self.builder.build_float_div(fa, fb, "fr").unwrap(),
            BinOp::Mod => self.builder.build_float_rem(fa, fb, "fr").unwrap(),
            _ => unreachable!(),
        };
        let rv = self.f64_val(fr);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort_at(line, col, &format!("'{kw}' needs numbers, got %s and %s"),
                      &[self.type_name(ta), self.type_name(tb)]);
    }

    fn build_cmp_fns(&self) {
        use inkwell::{FloatPredicate as FP, IntPredicate as IP};
        for (name, kw, ip, fp) in [
            ("verb_lt", "trails", IP::SLT, FP::OLT), ("verb_gt", "beats", IP::SGT, FP::OGT),
            ("verb_le", "atmost", IP::SLE, FP::OLE), ("verb_ge", "atleast", IP::SGE, FP::OGE),
        ] {
            let i32t = self.ctx.i32_type();
            let fnty = self.value_ty.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false);
            let f = self.module.add_function(name, fnty, None);
            let entry = self.ctx.append_basic_block(f, "entry");
            let int_bb = self.ctx.append_basic_block(f, "int");
            let chk_bb = self.ctx.append_basic_block(f, "chk");
            let flt_bb = self.ctx.append_basic_block(f, "flt");
            let err_bb = self.ctx.append_basic_block(f, "err");

            self.builder.position_at_end(entry);
            let a = f.get_nth_param(0).unwrap().into_struct_value();
            let b = f.get_nth_param(1).unwrap().into_struct_value();
            let line = f.get_nth_param(2).unwrap().into_int_value();
            let col = f.get_nth_param(3).unwrap().into_int_value();
            let (ta, pa) = (self.tag_of(a), self.payload_of(a));
            let (tb, pb) = (self.tag_of(b), self.payload_of(b));
            let i8t = self.ctx.i8_type();
            let ai = self.builder.build_int_compare(IP::EQ, ta, i8t.const_int(TAG_INT, false), "ai").unwrap();
            let bi = self.builder.build_int_compare(IP::EQ, tb, i8t.const_int(TAG_INT, false), "bi").unwrap();
            let both_int = self.builder.build_and(ai, bi, "bi2").unwrap();
            self.builder.build_conditional_branch(both_int, int_bb, chk_bb).unwrap();

            self.builder.position_at_end(int_bb);
            let r = self.builder.build_int_compare(ip, pa, pb, "c").unwrap();
            let rv = self.bool_val(r);
            self.builder.build_return(Some(&rv)).unwrap();

            self.builder.position_at_end(chk_bb);
            let an = self.is_numeric(ta);
            let bn = self.is_numeric(tb);
            let both = self.builder.build_and(an, bn, "bn2").unwrap();
            self.builder.build_conditional_branch(both, flt_bb, err_bb).unwrap();

            self.builder.position_at_end(flt_bb);
            let fa = self.to_f64(ta, pa);
            let fb = self.to_f64(tb, pb);
            let r = self.builder.build_float_compare(fp, fa, fb, "fc").unwrap();
            let rv = self.bool_val(r);
            self.builder.build_return(Some(&rv)).unwrap();

            self.builder.position_at_end(err_bb);
            self.abort_at(line, col, &format!("'{kw}' needs numbers, got %s and %s"),
                          &[self.type_name(ta), self.type_name(tb)]);
        }
    }

    fn build_eq_fn(&self) {
        use inkwell::{FloatPredicate as FP, IntPredicate as IP};
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function("verb_eq", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let same_bb = self.ctx.append_basic_block(f, "same");
        let raw_bb = self.ctx.append_basic_block(f, "raw");
        let feq_bb = self.ctx.append_basic_block(f, "feq");
        let seq_bb = self.ctx.append_basic_block(f, "seq");
        let diff_bb = self.ctx.append_basic_block(f, "diff");
        let mix_bb = self.ctx.append_basic_block(f, "mixed");
        let false_bb = self.ctx.append_basic_block(f, "no");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let same = self.builder.build_int_compare(IP::EQ, ta, tb, "same").unwrap();
        self.builder.build_conditional_branch(same, same_bb, diff_bb).unwrap();

        let i8t = self.ctx.i8_type();
        self.builder.position_at_end(same_bb);
        self.builder.build_switch(ta, raw_bb, &[
            (i8t.const_int(TAG_FLOAT, false), feq_bb),
            (i8t.const_int(TAG_STR, false), seq_bb),
        ]).unwrap();

        // nil/bool/int/closure: payload equality
        self.builder.position_at_end(raw_bb);
        let r = self.builder.build_int_compare(IP::EQ, pa, pb, "pe").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(feq_bb);
        let fa = self.builder.build_bit_cast(pa, self.ctx.f64_type(), "fa").unwrap().into_float_value();
        let fb = self.builder.build_bit_cast(pb, self.ctx.f64_type(), "fb").unwrap().into_float_value();
        let r = self.builder.build_float_compare(FP::OEQ, fa, fb, "fe").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(seq_bb);
        let sa = self.builder.build_int_to_ptr(pa, self.ptr_ty, "sa").unwrap();
        let sb = self.builder.build_int_to_ptr(pb, self.ptr_ty, "sb").unwrap();
        let c = self.call_named("strcmp", &[sa.into(), sb.into()]).unwrap().into_int_value();
        let r = self.builder.build_int_compare(IP::EQ, c, self.ctx.i32_type().const_zero(), "se").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        // different tags: numbers cross-compare, everything else unequal
        self.builder.position_at_end(diff_bb);
        let an = self.is_numeric(ta);
        let bn = self.is_numeric(tb);
        let both = self.builder.build_and(an, bn, "bn").unwrap();
        self.builder.build_conditional_branch(both, mix_bb, false_bb).unwrap();

        self.builder.position_at_end(mix_bb);
        let fa = self.to_f64(ta, pa);
        let fb = self.to_f64(tb, pb);
        let r = self.builder.build_float_compare(FP::OEQ, fa, fb, "me").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(false_bb);
        let rv = self.bool_val(self.ctx.bool_type().const_zero());
        self.builder.build_return(Some(&rv)).unwrap();
    }

    fn build_concat_fn(&self) {
        use inkwell::IntPredicate::*;
        let i32t = self.ctx.i32_type();
        let fnty = self.value_ty.fn_type(
            &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false);
        let f = self.module.add_function("verb_concat", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let i8t = self.ctx.i8_type();
        let as_ = self.builder.build_int_compare(EQ, ta, i8t.const_int(TAG_STR, false), "as").unwrap();
        let bs = self.builder.build_int_compare(EQ, tb, i8t.const_int(TAG_STR, false), "bs").unwrap();
        let both = self.builder.build_and(as_, bs, "both").unwrap();
        self.builder.build_conditional_branch(both, ok_bb, err_bb).unwrap();

        self.builder.position_at_end(ok_bb);
        let sa = self.builder.build_int_to_ptr(pa, self.ptr_ty, "sa").unwrap();
        let sb = self.builder.build_int_to_ptr(pb, self.ptr_ty, "sb").unwrap();
        let la = self.call_named("strlen", &[sa.into()]).unwrap().into_int_value();
        let lb = self.call_named("strlen", &[sb.into()]).unwrap().into_int_value();
        let loc = Some((line, col));
        let sum = self.checked_add_or_abort(la, lb, loc, false);
        let size = self.checked_add_or_abort(sum, self.ctx.i64_type().const_int(1, false), loc, false);
        let buf = self.call_named("verb_alloc", &[size.into()]).unwrap().into_pointer_value();
        self.call_named("strcpy", &[buf.into(), sa.into()]);
        self.call_named("strcat", &[buf.into(), sb.into()]);
        let bits = self.builder.build_ptr_to_int(buf, self.ctx.i64_type(), "bits").unwrap();
        let rv = self.make_val(TAG_STR, bits);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort_at(line, col, "'join' needs strings, got %s and %s",
                      &[self.type_name(ta), self.type_name(tb)]);
    }

    // ----- generated runtime helper: verb_str_len(value, i32, i32) -> value(int) -----

    /// `str_len(s)` — length in bytes of a Verb string. Aborts if `s` is
    /// not a string. Pure read of the literal/heap buffer; allocates nothing.
    fn build_str_len_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_str_len",
            self.value_ty.fn_type(&[self.value_ty.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let bad_bb = self.ctx.append_basic_block(f, "bad");
        self.builder.position_at_end(entry);
        let s = f.get_nth_param(0).unwrap().into_struct_value();
        let line = f.get_nth_param(1).unwrap().into_int_value();
        let col = f.get_nth_param(2).unwrap().into_int_value();
        let tag = self.tag_of(s);
        let is_str = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_STR, false), "isstr").unwrap();
        self.builder.build_conditional_branch(is_str, ok_bb, bad_bb).unwrap();

        self.builder.position_at_end(bad_bb);
        self.abort_at(line, col, "'str_len' needs a string, got %s", &[self.type_name(tag)]);

        self.builder.position_at_end(ok_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(s), self.ptr_ty, "sp").unwrap();
        let n = self.call_named("strlen", &[sp.into()]).unwrap().into_int_value();
        let rv = self.make_val(TAG_INT, n);
        self.builder.build_return(Some(&rv)).unwrap();
    }

    // ----- generated runtime helper: verb_str_index(s, sub, i32, i32) -> value(int) -----

    /// `str_index(s, sub)` — byte offset of the first occurrence of `sub`
    /// in `s`, or -1 if absent. Uses libc `strstr`; allocates nothing.
    fn build_str_index_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_str_index",
            self.value_ty.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let bad_bb = self.ctx.append_basic_block(f, "bad");
        self.builder.position_at_end(entry);
        let s = f.get_nth_param(0).unwrap().into_struct_value();
        let sub = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let i8t = self.ctx.i8_type();
        let ts = self.tag_of(s);
        let tsub = self.tag_of(sub);
        let ss = self.builder.build_int_compare(EQ, ts, i8t.const_int(TAG_STR, false), "ss").unwrap();
        let subs = self.builder.build_int_compare(EQ, tsub, i8t.const_int(TAG_STR, false), "subs").unwrap();
        let both = self.builder.build_and(ss, subs, "both").unwrap();
        self.builder.build_conditional_branch(both, ok_bb, bad_bb).unwrap();

        self.builder.position_at_end(bad_bb);
        self.abort_at(line, col, "'str_index' needs strings, got %s and %s",
                      &[self.type_name(ts), self.type_name(tsub)]);

        self.builder.position_at_end(ok_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(s), self.ptr_ty, "sp").unwrap();
        let subp = self.builder.build_int_to_ptr(self.payload_of(sub), self.ptr_ty, "subp").unwrap();
        let hit = self.call_named("strstr", &[sp.into(), subp.into()]).unwrap().into_pointer_value();
        let hit_int = self.builder.build_ptr_to_int(hit, i64t, "hit").unwrap();
        let is_null = self.builder.build_int_compare(EQ, hit_int, i64t.const_zero(), "isnull").unwrap();
        let sp_int = self.builder.build_ptr_to_int(sp, i64t, "spi").unwrap();
        let diff = self.builder.build_int_sub(hit_int, sp_int, "diff").unwrap();
        let neg1 = i64t.const_int((-1i64) as u64, true);
        let res = self.builder.build_select(is_null, neg1, diff, "idx").unwrap().into_int_value();
        let rv = self.make_val(TAG_INT, res);
        self.builder.build_return(Some(&rv)).unwrap();
    }

    // ----- generated runtime helper: verb_str_slice(s, start, end, i32, i32) -> value(str) -----

    /// `str_slice(s, start, end)` — a NEW heap string holding bytes
    /// `[start, end)` of `s`. Always `verb_alloc`s a fresh block (refcount
    /// 1) and `memcpy`s into it, so slicing a literal never aliases the
    /// literal's static (sentinel-headed) buffer. Bounds are checked
    /// (`0 <= start <= end <= len`) and violations `abort_at`.
    fn build_str_slice_fn(&self) {
        use inkwell::IntPredicate::{EQ, SLT, SGT};
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let i8t = self.ctx.i8_type();
        let f = self.module.add_function(
            "verb_str_slice",
            self.value_ty.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), self.value_ty.into(),
                  i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let typeok_bb = self.ctx.append_basic_block(f, "typeok");
        let badtype_bb = self.ctx.append_basic_block(f, "badtype");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let oob_bb = self.ctx.append_basic_block(f, "oob");
        self.builder.position_at_end(entry);
        let s = f.get_nth_param(0).unwrap().into_struct_value();
        let start_v = f.get_nth_param(1).unwrap().into_struct_value();
        let end_v = f.get_nth_param(2).unwrap().into_struct_value();
        let line = f.get_nth_param(3).unwrap().into_int_value();
        let col = f.get_nth_param(4).unwrap().into_int_value();
        let ts = self.tag_of(s);
        let is_str = self.builder.build_int_compare(EQ, ts, i8t.const_int(TAG_STR, false), "isstr").unwrap();
        let tstart = self.tag_of(start_v);
        let tend = self.tag_of(end_v);
        let si = self.builder.build_int_compare(EQ, tstart, i8t.const_int(TAG_INT, false), "si").unwrap();
        let ei = self.builder.build_int_compare(EQ, tend, i8t.const_int(TAG_INT, false), "ei").unwrap();
        let ints_ok = self.builder.build_and(si, ei, "intsok").unwrap();
        let all_ok = self.builder.build_and(is_str, ints_ok, "allok").unwrap();
        self.builder.build_conditional_branch(all_ok, typeok_bb, badtype_bb).unwrap();

        self.builder.position_at_end(badtype_bb);
        self.abort_at(line, col, "'str_slice' needs a string and two ints, got %s[%s..%s]",
                      &[self.type_name(ts), self.type_name(tstart), self.type_name(tend)]);

        self.builder.position_at_end(typeok_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(s), self.ptr_ty, "sp").unwrap();
        let len = self.call_named("strlen", &[sp.into()]).unwrap().into_int_value();
        let start = self.payload_of(start_v);
        let end = self.payload_of(end_v);
        let start_neg = self.builder.build_int_compare(SLT, start, i64t.const_zero(), "startneg").unwrap();
        let end_lt_start = self.builder.build_int_compare(SLT, end, start, "endltstart").unwrap();
        let end_gt_len = self.builder.build_int_compare(SGT, end, len, "endgtlen").unwrap();
        let bad1 = self.builder.build_or(start_neg, end_lt_start, "bad1").unwrap();
        let bad = self.builder.build_or(bad1, end_gt_len, "bad").unwrap();
        self.builder.build_conditional_branch(bad, oob_bb, ok_bb).unwrap();

        self.builder.position_at_end(oob_bb);
        self.abort_at(line, col, "str_slice out of bounds: start=%lld end=%lld len=%lld",
                      &[start.into(), end.into(), len.into()]);

        self.builder.position_at_end(ok_bb);
        let n = self.builder.build_int_sub(end, start, "n").unwrap();
        let size = self.builder.build_int_add(n, i64t.const_int(1, false), "sz").unwrap();
        let buf = self.call_named("verb_alloc", &[size.into()]).unwrap().into_pointer_value();
        let src = unsafe { self.builder.build_in_bounds_gep(i8t, sp, &[start], "src") }.unwrap();
        self.call_named("memcpy", &[buf.into(), src.into(), n.into()]);
        let term = unsafe { self.builder.build_in_bounds_gep(i8t, buf, &[n], "term") }.unwrap();
        self.builder.build_store(term, i8t.const_zero()).unwrap();
        let bits = self.builder.build_ptr_to_int(buf, i64t, "bits").unwrap();
        let rv = self.make_val(TAG_STR, bits);
        self.builder.build_return(Some(&rv)).unwrap();
    }

    // ----- generated runtime helper: verb_str_split(s, sep, i32, i32) -> value(array) -----

    /// `str_split(s, sep)` — a NEW array whose elements are fresh heap
    /// strings, the pieces of `s` between occurrences of `sep`. The array
    /// header, its element buffer, and every piece are separate `verb_alloc`
    /// blocks (refcount 1), so the existing GC array cascade releases them
    /// all — no new GC plumbing. An empty separator yields a single-element
    /// array holding a fresh copy of `s`.
    fn build_str_split_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let i8t = self.ctx.i8_type();
        let f = self.module.add_function(
            "verb_str_split",
            self.value_ty.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let typeok_bb = self.ctx.append_basic_block(f, "typeok");
        let badtype_bb = self.ctx.append_basic_block(f, "badtype");
        let single_bb = self.ctx.append_basic_block(f, "single");
        let countinit_bb = self.ctx.append_basic_block(f, "countinit");
        let count_cond_bb = self.ctx.append_basic_block(f, "count.cond");
        let count_body_bb = self.ctx.append_basic_block(f, "count.body");
        let count_done_bb = self.ctx.append_basic_block(f, "count.done");
        let fill_cond_bb = self.ctx.append_basic_block(f, "fill.cond");
        let fill_next_bb = self.ctx.append_basic_block(f, "fill.next");
        let fill_done_bb = self.ctx.append_basic_block(f, "fill.done");

        self.builder.position_at_end(entry);
        let s = f.get_nth_param(0).unwrap().into_struct_value();
        let sep = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let ts = self.tag_of(s);
        let tsep = self.tag_of(sep);
        let ss = self.builder.build_int_compare(EQ, ts, i8t.const_int(TAG_STR, false), "ss").unwrap();
        let seps = self.builder.build_int_compare(EQ, tsep, i8t.const_int(TAG_STR, false), "seps").unwrap();
        let both = self.builder.build_and(ss, seps, "both").unwrap();
        self.builder.build_conditional_branch(both, typeok_bb, badtype_bb).unwrap();

        self.builder.position_at_end(badtype_bb);
        self.abort_at(line, col, "'str_split' needs strings, got %s and %s",
                      &[self.type_name(ts), self.type_name(tsep)]);

        // Loop bookkeeping (allocas in entry-block idiom via entry_alloca).
        let curp = self.entry_alloca(self.ptr_ty.into(), "curp");
        let countp = self.entry_alloca(i64t.into(), "countp");
        let ip = self.entry_alloca(i64t.into(), "ip");
        let hdrp = self.entry_alloca(self.ptr_ty.into(), "hdrp");
        let elemsp_slot = self.entry_alloca(self.ptr_ty.into(), "elemsslot");

        self.builder.position_at_end(typeok_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(s), self.ptr_ty, "sp").unwrap();
        let sepp = self.builder.build_int_to_ptr(self.payload_of(sep), self.ptr_ty, "sepp").unwrap();
        let seplen = self.call_named("strlen", &[sepp.into()]).unwrap().into_int_value();
        let sep_empty = self.builder.build_int_compare(EQ, seplen, i64t.const_zero(), "sepempty").unwrap();
        self.builder.build_conditional_branch(sep_empty, single_bb, countinit_bb).unwrap();

        // --- empty-separator fast path: array of one fresh copy of s ---
        self.builder.position_at_end(single_bb);
        let hdr1 = self.call_named("verb_alloc", &[i64t.const_int(24, false).into()])
            .unwrap().into_pointer_value();
        let elems1 = self.call_named("verb_alloc", &[i64t.const_int(16, false).into()])
            .unwrap().into_pointer_value();
        let slen = self.call_named("strlen", &[sp.into()]).unwrap().into_int_value();
        let sz1 = self.builder.build_int_add(slen, i64t.const_int(1, false), "sz1").unwrap();
        let buf1 = self.call_named("verb_alloc", &[sz1.into()]).unwrap().into_pointer_value();
        self.call_named("memcpy", &[buf1.into(), sp.into(), slen.into()]);
        let term1 = unsafe { self.builder.build_in_bounds_gep(i8t, buf1, &[slen], "term1") }.unwrap();
        self.builder.build_store(term1, i8t.const_zero()).unwrap();
        let strv1 = self.make_val(TAG_STR, self.builder.build_ptr_to_int(buf1, i64t, "b1").unwrap());
        let slot1 = unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, elems1, &[i64t.const_zero()], "slot1")
        }.unwrap();
        self.builder.build_store(slot1, strv1).unwrap();
        self.store_array_header(hdr1, i64t.const_int(1, false), elems1);
        let arr1 = self.make_val(TAG_ARRAY, self.builder.build_ptr_to_int(hdr1, i64t, "a1").unwrap());
        self.builder.build_return(Some(&arr1)).unwrap();

        // --- pass 1: count pieces (= 1 + number of separator occurrences) ---
        self.builder.position_at_end(countinit_bb);
        self.builder.build_store(countp, i64t.const_int(1, false)).unwrap();
        self.builder.build_store(curp, sp).unwrap();
        self.builder.build_unconditional_branch(count_cond_bb).unwrap();

        self.builder.position_at_end(count_cond_bb);
        let cur1 = self.builder.build_load(self.ptr_ty, curp, "cur1").unwrap().into_pointer_value();
        let hit1 = self.call_named("strstr", &[cur1.into(), sepp.into()]).unwrap().into_pointer_value();
        let hit1_int = self.builder.build_ptr_to_int(hit1, i64t, "hit1i").unwrap();
        let isnull1 = self.builder.build_int_compare(EQ, hit1_int, i64t.const_zero(), "isnull1").unwrap();
        self.builder.build_conditional_branch(isnull1, count_done_bb, count_body_bb).unwrap();

        self.builder.position_at_end(count_body_bb);
        let cnt = self.builder.build_load(i64t, countp, "cnt").unwrap().into_int_value();
        let cnt1 = self.builder.build_int_add(cnt, i64t.const_int(1, false), "cnt1").unwrap();
        self.builder.build_store(countp, cnt1).unwrap();
        let next1 = unsafe { self.builder.build_in_bounds_gep(i8t, hit1, &[seplen], "next1") }.unwrap();
        self.builder.build_store(curp, next1).unwrap();
        self.builder.build_unconditional_branch(count_cond_bb).unwrap();

        // --- allocate the array, then pass 2: fill it ---
        self.builder.position_at_end(count_done_bb);
        let count = self.builder.build_load(i64t, countp, "count").unwrap().into_int_value();
        let hdr = self.call_named("verb_alloc", &[i64t.const_int(24, false).into()])
            .unwrap().into_pointer_value();
        let bytes = self.builder.build_int_mul(count, i64t.const_int(16, false), "bytes").unwrap();
        let elems = self.malloc_bytes_dyn(bytes);
        self.builder.build_store(hdrp, hdr).unwrap();
        self.builder.build_store(elemsp_slot, elems).unwrap();
        self.builder.build_store(ip, i64t.const_zero()).unwrap();
        self.builder.build_store(curp, sp).unwrap();
        self.builder.build_unconditional_branch(fill_cond_bb).unwrap();

        self.builder.position_at_end(fill_cond_bb);
        let cur = self.builder.build_load(self.ptr_ty, curp, "cur").unwrap().into_pointer_value();
        let hit = self.call_named("strstr", &[cur.into(), sepp.into()]).unwrap().into_pointer_value();
        let hit_int = self.builder.build_ptr_to_int(hit, i64t, "hiti").unwrap();
        let isnull = self.builder.build_int_compare(EQ, hit_int, i64t.const_zero(), "isnull").unwrap();
        let cur_int = self.builder.build_ptr_to_int(cur, i64t, "curi").unwrap();
        let curlen = self.call_named("strlen", &[cur.into()]).unwrap().into_int_value();
        let end_if_null = self.builder.build_int_add(cur_int, curlen, "endnull").unwrap();
        let end_int = self.builder.build_select(isnull, end_if_null, hit_int, "endi").unwrap().into_int_value();
        let piece = self.builder.build_int_sub(end_int, cur_int, "piece").unwrap();
        let psz = self.builder.build_int_add(piece, i64t.const_int(1, false), "psz").unwrap();
        let buf = self.call_named("verb_alloc", &[psz.into()]).unwrap().into_pointer_value();
        self.call_named("memcpy", &[buf.into(), cur.into(), piece.into()]);
        let term = unsafe { self.builder.build_in_bounds_gep(i8t, buf, &[piece], "term") }.unwrap();
        self.builder.build_store(term, i8t.const_zero()).unwrap();
        let strv = self.make_val(TAG_STR, self.builder.build_ptr_to_int(buf, i64t, "bufi").unwrap());
        let i = self.builder.build_load(i64t, ip, "i").unwrap().into_int_value();
        let elems_now = self.builder.build_load(self.ptr_ty, elemsp_slot, "elemsnow").unwrap().into_pointer_value();
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems_now, &[i], "slot") }.unwrap();
        self.builder.build_store(slot, strv).unwrap();
        let i1 = self.builder.build_int_add(i, i64t.const_int(1, false), "i1").unwrap();
        self.builder.build_store(ip, i1).unwrap();
        self.builder.build_conditional_branch(isnull, fill_done_bb, fill_next_bb).unwrap();

        self.builder.position_at_end(fill_next_bb);
        let next = unsafe { self.builder.build_in_bounds_gep(i8t, hit, &[seplen], "next") }.unwrap();
        self.builder.build_store(curp, next).unwrap();
        self.builder.build_unconditional_branch(fill_cond_bb).unwrap();

        self.builder.position_at_end(fill_done_bb);
        let hdr_f = self.builder.build_load(self.ptr_ty, hdrp, "hdrf").unwrap().into_pointer_value();
        let elems_f = self.builder.build_load(self.ptr_ty, elemsp_slot, "elemsf").unwrap().into_pointer_value();
        self.store_array_header(hdr_f, count, elems_f);
        let arr = self.make_val(TAG_ARRAY, self.builder.build_ptr_to_int(hdr_f, i64t, "arri").unwrap());
        self.builder.build_return(Some(&arr)).unwrap();
    }

    /// Writes {len, cap=len, elems} into an array header block laid out like
    /// `array_ty` (see `Expr::ArrayLit`).
    fn store_array_header(&self, hdr: PointerValue<'ctx>, len: IntValue<'ctx>, elems: PointerValue<'ctx>) {
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        self.builder.build_store(lenp, len).unwrap();
        let capp = self.builder.build_struct_gep(self.array_ty, hdr, 1, "capp").unwrap();
        self.builder.build_store(capp, len).unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        self.builder.build_store(elemsp, elems).unwrap();
    }

    fn build_neg_fn(&self) {
        use inkwell::IntPredicate::*;
        let i32t = self.ctx.i32_type();
        let fnty = self.value_ty.fn_type(
            &[self.value_ty.into(), i32t.into(), i32t.into()], false);
        let f = self.module.add_function("verb_neg", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chk");
        let flt_bb = self.ctx.append_basic_block(f, "flt");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let line = f.get_nth_param(1).unwrap().into_int_value();
        let col = f.get_nth_param(2).unwrap().into_int_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let i8t = self.ctx.i8_type();
        let isi = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_INT, false), "isi").unwrap();
        self.builder.build_conditional_branch(isi, int_bb, chk_bb).unwrap();

        self.builder.position_at_end(int_bb);
        let n = self.builder.build_int_neg(p, "n").unwrap();
        let rv = self.make_val(TAG_INT, n);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(chk_bb);
        let isf = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_FLOAT, false), "isf").unwrap();
        self.builder.build_conditional_branch(isf, flt_bb, err_bb).unwrap();

        self.builder.position_at_end(flt_bb);
        let fv = self.builder.build_bit_cast(p, self.ctx.f64_type(), "f").unwrap().into_float_value();
        let n = self.builder.build_float_neg(fv, "fn").unwrap();
        let rv = self.f64_val(n);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort_at(line, col, "'neg' needs a number, got %s", &[self.type_name(t)]);
    }

    // ----- generated runtime helper: verb_check_call(value, argc) -> closure ptr -----

    /// Aborts unless `v` is a closure whose arity equals `argc`; returns the closure struct ptr.
    fn build_check_call_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_check_call",
            self.ptr_ty.fn_type(
                &[self.value_ty.into(), i64t.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let argc = f.get_nth_param(1).unwrap().into_int_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();

        let arity_bb = self.ctx.append_basic_block(f, "arity");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let notfn_bb = self.ctx.append_basic_block(f, "notfn");
        let badarity_bb = self.ctx.append_basic_block(f, "badarity");

        let tag = self.tag_of(v);
        let is_clos = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_CLOSURE, false), "isclos").unwrap();
        self.builder.build_conditional_branch(is_clos, arity_bb, notfn_bb).unwrap();

        self.builder.position_at_end(arity_bb);
        let p = self.builder.build_int_to_ptr(self.payload_of(v), self.ptr_ty, "cp").unwrap();
        let ap = self.builder.build_struct_gep(self.closure_ty, p, 1, "ap").unwrap();
        let arity = self.builder.build_load(i64t, ap, "arity").unwrap().into_int_value();
        let ok = self.builder.build_int_compare(EQ, arity, argc, "arityok").unwrap();
        self.builder.build_conditional_branch(ok, ok_bb, badarity_bb).unwrap();

        self.builder.position_at_end(ok_bb);
        self.builder.build_return(Some(&p)).unwrap();

        self.builder.position_at_end(notfn_bb);
        let tag = self.tag_of(v);
        self.abort_at(line, col, "can only call functions, got %s", &[self.type_name(tag)]);

        self.builder.position_at_end(badarity_bb);
        self.abort_at(line, col, "wrong number of arguments: expected %lld, got %lld",
                      &[arity.into(), argc.into()]);
    }

    // ----- generated runtime helper: verb_array_len(value, i32, i32) -> value -----

    fn build_array_len_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_len",
            self.value_ty.fn_type(&[self.value_ty.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let line = f.get_nth_param(1).unwrap().into_int_value();
        let col = f.get_nth_param(2).unwrap().into_int_value();

        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let bad_bb = self.ctx.append_basic_block(f, "badtype");
        let tag = self.tag_of(arr);
        let is_arr = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_ARRAY, false), "isarr").unwrap();
        self.builder.build_conditional_branch(is_arr, ok_bb, bad_bb).unwrap();

        self.builder.position_at_end(bad_bb);
        self.abort_at(line, col, "'len' needs an array, got %s", &[self.type_name(tag)]);

        self.builder.position_at_end(ok_bb);
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        let len = self.builder.build_load(self.ctx.i64_type(), lenp, "len").unwrap().into_int_value();
        let rv = self.make_val(TAG_INT, len);
        self.builder.build_return(Some(&rv)).unwrap();
    }

    // ----- shared: verb_array_check(value arr, value idx, i32 line, i32 col, ptr opname) -> i64 (validated index) -----

    /// Aborts unless `arr` is an array and `idx` is an int within bounds;
    /// returns the validated index as a plain i64. `opname` is a %s-ready
    /// C string ("get" or "set") used in error messages.
    fn build_array_check_fn(&self) {
        use inkwell::IntPredicate::{EQ, SLT, SGE};
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_check",
            i64t.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into(), self.ptr_ty.into()],
                false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let idx = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let opname = f.get_nth_param(4).unwrap().into_pointer_value();

        let arr_ok_bb = self.ctx.append_basic_block(f, "arrok");
        let arr_bad_bb = self.ctx.append_basic_block(f, "arrbad");
        let atag = self.tag_of(arr);
        let is_arr = self.builder.build_int_compare(
            EQ, atag, self.ctx.i8_type().const_int(TAG_ARRAY, false), "isarr").unwrap();
        self.builder.build_conditional_branch(is_arr, arr_ok_bb, arr_bad_bb).unwrap();

        self.builder.position_at_end(arr_bad_bb);
        self.abort_at(line, col, "'%s' needs an array, got %s", &[opname.into(), self.type_name(atag)]);

        self.builder.position_at_end(arr_ok_bb);
        let idx_ok_bb = self.ctx.append_basic_block(f, "idxok");
        let idx_bad_bb = self.ctx.append_basic_block(f, "idxbad");
        let itag = self.tag_of(idx);
        let is_int = self.builder.build_int_compare(
            EQ, itag, self.ctx.i8_type().const_int(TAG_INT, false), "isint").unwrap();
        self.builder.build_conditional_branch(is_int, idx_ok_bb, idx_bad_bb).unwrap();

        self.builder.position_at_end(idx_bad_bb);
        self.abort_at(line, col, "'%s' needs an int index, got %s", &[opname.into(), self.type_name(itag)]);

        self.builder.position_at_end(idx_ok_bb);
        let i = self.payload_of(idx);
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        let len = self.builder.build_load(i64t, lenp, "len").unwrap().into_int_value();
        let too_low = self.builder.build_int_compare(SLT, i, i64t.const_zero(), "toolow").unwrap();
        let too_high = self.builder.build_int_compare(SGE, i, len, "toohigh").unwrap();
        let out_of_range = self.builder.build_or(too_low, too_high, "oor").unwrap();
        let inrange_bb = self.ctx.append_basic_block(f, "inrange");
        let oor_bb = self.ctx.append_basic_block(f, "oor");
        self.builder.build_conditional_branch(out_of_range, oor_bb, inrange_bb).unwrap();

        self.builder.position_at_end(oor_bb);
        self.abort_at(line, col, "index %lld out of bounds for array of length %lld", &[i.into(), len.into()]);

        self.builder.position_at_end(inrange_bb);
        self.builder.build_return(Some(&i)).unwrap();
    }

    // ----- generated runtime helper: verb_array_get(arr, idx, line, col) -> value -----

    fn build_array_get_fn(&self) {
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_get",
            self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let idx = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();

        let opname = self.cstr("get");
        let i = self.call_named("verb_array_check", &[arr.into(), idx.into(), line.into(), col.into(), opname.into()])
            .unwrap().into_int_value();
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        let v = self.builder.build_load(self.value_ty, slot, "v").unwrap().into_struct_value();
        // The array's own slot keeps its reference; `get` hands back an
        // independent copy, mirroring Expr::Var's retain-on-load.
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_return(Some(&v)).unwrap();
    }

    // ----- generated runtime helper: verb_array_set(arr, idx, v, line, col) -> value (returns v) -----

    fn build_array_set_fn(&self) {
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_set",
            self.value_ty.fn_type(
                &[self.value_ty.into(), self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()],
                false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let idx = f.get_nth_param(1).unwrap().into_struct_value();
        let v = f.get_nth_param(2).unwrap().into_struct_value();
        let line = f.get_nth_param(3).unwrap().into_int_value();
        let col = f.get_nth_param(4).unwrap().into_int_value();

        let opname = self.cstr("set");
        let i = self.call_named("verb_array_check", &[arr.into(), idx.into(), line.into(), col.into(), opname.into()])
            .unwrap().into_int_value();
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        // `v` (the caller's owned temporary) is about to have two homes --
        // the array slot and the returned value -- where before it had
        // one. One retain covers the second home; the slot's copy is the
        // transfer of `v`'s original ownership (no separate op needed for
        // that half).
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_store(slot, v).unwrap();
        self.builder.build_return(Some(&v)).unwrap();
    }

    // ----- generated runtime helper: verb_array_push(arr, v, line, col) -> value (nil) -----

    fn build_array_push_fn(&self) {
        use inkwell::IntPredicate::{EQ, SLT};
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_push",
            self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()], false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let v = f.get_nth_param(1).unwrap().into_struct_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();

        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let bad_bb = self.ctx.append_basic_block(f, "badtype");
        let atag = self.tag_of(arr);
        let is_arr = self.builder.build_int_compare(
            EQ, atag, self.ctx.i8_type().const_int(TAG_ARRAY, false), "isarr").unwrap();
        self.builder.build_conditional_branch(is_arr, ok_bb, bad_bb).unwrap();

        self.builder.position_at_end(bad_bb);
        self.abort_at(line, col, "'push' needs an array, got %s", &[self.type_name(atag)]);

        self.builder.position_at_end(ok_bb);
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        let capp = self.builder.build_struct_gep(self.array_ty, hdr, 1, "capp").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        let len = self.builder.build_load(i64t, lenp, "len").unwrap().into_int_value();
        let cap = self.builder.build_load(i64t, capp, "cap").unwrap().into_int_value();

        let grow_bb = self.ctx.append_basic_block(f, "grow");
        let after_grow_bb = self.ctx.append_basic_block(f, "afterGrow");
        let need_grow = self.builder.build_int_compare(EQ, len, cap, "needgrow").unwrap();
        self.builder.build_conditional_branch(need_grow, grow_bb, after_grow_bb).unwrap();

        self.builder.position_at_end(grow_bb);
        let one = i64t.const_int(1, false);
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let is_zero = self.builder.build_int_compare(EQ, cap, i64t.const_zero(), "capzero").unwrap();
        let loc = Some((line, col));
        let doubled = self.checked_mul_or_abort(cap, i64t.const_int(2, false), loc, false);
        let new_cap = self.builder.build_select(is_zero, one, doubled, "newcap").unwrap().into_int_value();
        let new_bytes = self.checked_mul_or_abort(new_cap, i64t.const_int(16, false), loc, false);
        let new_elems = self.malloc_bytes_dyn(new_bytes);

        let idxp = self.entry_alloca(i64t.into(), "cpidx");
        self.builder.build_store(idxp, i64t.const_zero()).unwrap();
        let cp_cond = self.ctx.append_basic_block(f, "cp.cond");
        let cp_body = self.ctx.append_basic_block(f, "cp.body");
        let cp_end = self.ctx.append_basic_block(f, "cp.end");
        self.builder.build_unconditional_branch(cp_cond).unwrap();

        self.builder.position_at_end(cp_cond);
        let i = self.builder.build_load(i64t, idxp, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(SLT, i, len, "more").unwrap();
        self.builder.build_conditional_branch(more, cp_body, cp_end).unwrap();

        self.builder.position_at_end(cp_body);
        let src = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "src") }.unwrap();
        let dst = unsafe { self.builder.build_in_bounds_gep(self.value_ty, new_elems, &[i], "dst") }.unwrap();
        let elemv = self.builder.build_load(self.value_ty, src, "elemv").unwrap();
        self.builder.build_store(dst, elemv).unwrap();
        let next = self.builder.build_int_add(i, one, "next").unwrap();
        self.builder.build_store(idxp, next).unwrap();
        self.builder.build_unconditional_branch(cp_cond).unwrap();

        self.builder.position_at_end(cp_end);
        let old_elems_addr = self.builder.build_ptr_to_int(elems, i64t, "old_elems_addr").unwrap();
        let old_elems_null = self.builder.build_int_compare(
            EQ, old_elems_addr, i64t.const_zero(), "old_elems_null").unwrap();
        let free_old_bb = self.ctx.append_basic_block(f, "free_old_elems");
        let skip_free_old_bb = self.ctx.append_basic_block(f, "skip_free_old_elems");
        self.builder.build_conditional_branch(old_elems_null, skip_free_old_bb, free_old_bb).unwrap();

        self.builder.position_at_end(free_old_bb);
        self.dec_live_counter();
        self.call_named("free", &[self.header_ptr(elems).into()]);
        self.builder.build_unconditional_branch(skip_free_old_bb).unwrap();

        self.builder.position_at_end(skip_free_old_bb);
        self.builder.build_store(capp, new_cap).unwrap();
        self.builder.build_store(elemsp, new_elems).unwrap();
        self.builder.build_unconditional_branch(after_grow_bb).unwrap();

        self.builder.position_at_end(after_grow_bb);
        let elems2 = self.builder.build_load(self.ptr_ty, elemsp, "elems2").unwrap().into_pointer_value();
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems2, &[len], "slot") }.unwrap();
        self.builder.build_store(slot, v).unwrap();
        let newlen = self.builder.build_int_add(len, one, "newlen").unwrap();
        self.builder.build_store(lenp, newlen).unwrap();
        let nilv = self.nil_val();
        self.builder.build_return(Some(&nilv)).unwrap();
    }

    // ----- generated runtime helper: verb_array_pop(arr, line, col) -> value -----

    fn build_array_pop_fn(&self) {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_array_pop",
            self.value_ty.fn_type(&[self.value_ty.into(), i32t.into(), i32t.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let arr = f.get_nth_param(0).unwrap().into_struct_value();
        let line = f.get_nth_param(1).unwrap().into_int_value();
        let col = f.get_nth_param(2).unwrap().into_int_value();

        let arrok_bb = self.ctx.append_basic_block(f, "arrok");
        let arrbad_bb = self.ctx.append_basic_block(f, "arrbad");
        let atag = self.tag_of(arr);
        let is_arr = self.builder.build_int_compare(
            EQ, atag, self.ctx.i8_type().const_int(TAG_ARRAY, false), "isarr").unwrap();
        self.builder.build_conditional_branch(is_arr, arrok_bb, arrbad_bb).unwrap();

        self.builder.position_at_end(arrbad_bb);
        self.abort_at(line, col, "'pop' needs an array, got %s", &[self.type_name(atag)]);

        self.builder.position_at_end(arrok_bb);
        let hdr = self.builder.build_int_to_ptr(self.payload_of(arr), self.ptr_ty, "hdr").unwrap();
        let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
        let len = self.builder.build_load(i64t, lenp, "len").unwrap().into_int_value();

        let nonempty_bb = self.ctx.append_basic_block(f, "nonempty");
        let empty_bb = self.ctx.append_basic_block(f, "empty");
        let is_empty = self.builder.build_int_compare(EQ, len, i64t.const_zero(), "isempty").unwrap();
        self.builder.build_conditional_branch(is_empty, empty_bb, nonempty_bb).unwrap();

        self.builder.position_at_end(empty_bb);
        self.abort_at(line, col, "pop from empty array", &[]);

        self.builder.position_at_end(nonempty_bb);
        let newlen = self.builder.build_int_sub(len, i64t.const_int(1, false), "newlen").unwrap();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[newlen], "slot") }.unwrap();
        let v = self.builder.build_load(self.value_ty, slot, "v").unwrap().into_struct_value();
        self.builder.build_store(lenp, newlen).unwrap();
        self.builder.build_return(Some(&v)).unwrap();
    }

    /// Runtime helper: verb_retain_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag (string, closure, array, map).
    /// Static string literals (sentinel header) are skipped -- immortal,
    /// count never moves.
    fn build_retain_value_fn(&self) {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_retain_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let str_bb = self.ctx.append_basic_block(f, "str");
        let str_bump_bb = self.ctx.append_basic_block(f, "str.bump");
        let heap_check_bb = self.ctx.append_basic_block(f, "heap.check");
        let heap_bump_bb = self.ctx.append_basic_block(f, "heap.bump");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, heap_check_bb).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(p, self.ptr_ty, "sp").unwrap();
        let shdr = self.header_ptr(sp);
        let scur = self.builder.build_load(i64t, shdr, "scur").unwrap().into_int_value();
        let is_static = self.builder.build_int_compare(
            EQ, scur, i64t.const_int(GC_STATIC_SENTINEL as u64, true), "is_static").unwrap();
        self.builder.build_conditional_branch(is_static, done_bb, str_bump_bb).unwrap();

        self.builder.position_at_end(str_bump_bb);
        let snext = self.builder.build_int_add(scur, i64t.const_int(1, false), "snext").unwrap();
        self.builder.build_store(shdr, snext).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // closure/array/map all share the same "always heap, always just
        // bump the header" behavior for retain -- only release (Step 4)
        // needs different cascade logic per tag.
        self.builder.position_at_end(heap_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        let is_arr = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ARRAY, false), "is_arr").unwrap();
        let is_map = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_MAP, false), "is_map").unwrap();
        let is_struct = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STRUCT, false), "is_struct").unwrap();
        let is_enum = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ENUM, false), "is_enum").unwrap();
        let is_clos_or_arr = self.builder.build_or(is_clos, is_arr, "is_clos_or_arr").unwrap();
        let is_map_or_struct = self.builder.build_or(is_map, is_struct, "is_map_or_struct").unwrap();
        let is_heap0 = self.builder.build_or(is_clos_or_arr, is_map_or_struct, "is_heap0").unwrap();
        let is_heap = self.builder.build_or(is_heap0, is_enum, "is_heap").unwrap();
        self.builder.build_conditional_branch(is_heap, heap_bump_bb, done_bb).unwrap();

        self.builder.position_at_end(heap_bump_bb);
        let hp = self.builder.build_int_to_ptr(p, self.ptr_ty, "hp").unwrap();
        let hhdr = self.header_ptr(hp);
        let hcur = self.builder.build_load(i64t, hhdr, "hcur").unwrap().into_int_value();
        let hnext = self.builder.build_int_add(hcur, i64t.const_int(1, false), "hnext").unwrap();
        self.builder.build_store(hhdr, hnext).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }

    /// Runtime helper: verb_release_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag; on those, decrements the header
    /// and, at zero, cascades per-tag before freeing:
    /// - STR: no cascade, just free (skip entirely if static sentinel).
    /// - CLOSURE: release the capture env (cascading into captured values and
    ///   freeing the env block when its own refcount hits zero), then free the
    ///   closure block. A non-capturing closure has a null env: block-only.
    /// - ARRAY: release every element 0..len (cascading into any
    ///   heap-owned element), free `elems`, free the header.
    /// - MAP: call `verb_map_destroy_contents` (defined in
    ///   runtime/verb_map.cpp) to cascade-release every key/value and run
    ///   the map's C++ destructor, then free the header here (the one
    ///   place every heap kind's header actually gets `free`d).
    fn build_release_value_fn(&self) {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_release_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let str_bb = self.ctx.append_basic_block(f, "str");
        let str_live_bb = self.ctx.append_basic_block(f, "str.live");
        let str_free_bb = self.ctx.append_basic_block(f, "str.free");
        let clos_check_bb = self.ctx.append_basic_block(f, "clos.check");
        let clos_bb = self.ctx.append_basic_block(f, "clos");
        let clos_dec_bb = self.ctx.append_basic_block(f, "clos.dec");
        let clos_free_bb = self.ctx.append_basic_block(f, "clos.free");
        let clos_env_live_bb = self.ctx.append_basic_block(f, "clos.env.live");
        let clos_env_free_bb = self.ctx.append_basic_block(f, "clos.env.free");
        let clos_env_loop_cond_bb = self.ctx.append_basic_block(f, "clos.env.loop.cond");
        let clos_env_loop_body_bb = self.ctx.append_basic_block(f, "clos.env.loop.body");
        let clos_env_loop_end_bb = self.ctx.append_basic_block(f, "clos.env.loop.end");
        let clos_block_free_bb = self.ctx.append_basic_block(f, "clos.block.free");
        let arr_check_bb = self.ctx.append_basic_block(f, "arr.check");
        let arr_bb = self.ctx.append_basic_block(f, "arr");
        let arr_dec_bb = self.ctx.append_basic_block(f, "arr.dec");
        let arr_free_bb = self.ctx.append_basic_block(f, "arr.free");
        let arr_loop_cond_bb = self.ctx.append_basic_block(f, "arr.loop.cond");
        let arr_loop_body_bb = self.ctx.append_basic_block(f, "arr.loop.body");
        let arr_loop_end_bb = self.ctx.append_basic_block(f, "arr.loop.end");
        let arr_free_elems_bb = self.ctx.append_basic_block(f, "arr.free_elems");
        let arr_skip_elems_bb = self.ctx.append_basic_block(f, "arr.skip_elems");
        let map_check_bb = self.ctx.append_basic_block(f, "map.check");
        let map_bb = self.ctx.append_basic_block(f, "map");
        let map_dec_bb = self.ctx.append_basic_block(f, "map.dec");
        let map_free_bb = self.ctx.append_basic_block(f, "map.free");
        let struct_check_bb = self.ctx.append_basic_block(f, "struct.check");
        let struct_bb = self.ctx.append_basic_block(f, "struct");
        let struct_dec_bb = self.ctx.append_basic_block(f, "struct.dec");
        let struct_free_bb = self.ctx.append_basic_block(f, "struct.free");
        let struct_loop_cond_bb = self.ctx.append_basic_block(f, "struct.loop.cond");
        let struct_loop_body_bb = self.ctx.append_basic_block(f, "struct.loop.body");
        let struct_loop_end_bb = self.ctx.append_basic_block(f, "struct.loop.end");
        let enum_check_bb = self.ctx.append_basic_block(f, "enum.check");
        let enum_bb = self.ctx.append_basic_block(f, "enum");
        let enum_dec_bb = self.ctx.append_basic_block(f, "enum.dec");
        let enum_free_bb = self.ctx.append_basic_block(f, "enum.free");
        let enum_loop_cond_bb = self.ctx.append_basic_block(f, "enum.loop.cond");
        let enum_loop_body_bb = self.ctx.append_basic_block(f, "enum.loop.body");
        let enum_loop_end_bb = self.ctx.append_basic_block(f, "enum.loop.end");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, clos_check_bb).unwrap();

        // ----- string -----
        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(p, self.ptr_ty, "sp").unwrap();
        let shdr = self.header_ptr(sp);
        let scur = self.builder.build_load(i64t, shdr, "scur").unwrap().into_int_value();
        let is_static = self.builder.build_int_compare(
            EQ, scur, i64t.const_int(GC_STATIC_SENTINEL as u64, true), "is_static").unwrap();
        self.builder.build_conditional_branch(is_static, done_bb, str_live_bb).unwrap();

        self.builder.position_at_end(str_live_bb);
        let snext = self.builder.build_int_sub(scur, i64t.const_int(1, false), "snext").unwrap();
        self.builder.build_store(shdr, snext).unwrap();
        let szero = self.builder.build_int_compare(EQ, snext, i64t.const_zero(), "szero").unwrap();
        self.builder.build_conditional_branch(szero, str_free_bb, done_bb).unwrap();

        self.builder.position_at_end(str_free_bb);
        self.dec_live_counter();
        self.call_named("free", &[shdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- closure: release capture env (if any), then free the block -----
        self.builder.position_at_end(clos_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        self.builder.build_conditional_branch(is_clos, clos_bb, arr_check_bb).unwrap();

        self.builder.position_at_end(clos_bb);
        let cp = self.builder.build_int_to_ptr(p, self.ptr_ty, "cp").unwrap();
        let chdr = self.header_ptr(cp);
        let ccur = self.builder.build_load(i64t, chdr, "ccur").unwrap().into_int_value();
        let cnext = self.builder.build_int_sub(ccur, i64t.const_int(1, false), "cnext").unwrap();
        self.builder.build_store(chdr, cnext).unwrap();
        let czero = self.builder.build_int_compare(EQ, cnext, i64t.const_zero(), "czero").unwrap();
        self.builder.build_conditional_branch(czero, clos_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(clos_dec_bb);
        self.builder.build_unconditional_branch(clos_free_bb).unwrap();

        // At refcount zero, reclaim the capture env before the closure block.
        // The env is refcounted at closure-block granularity (see `make_closure`
        // / `retain_env`): decrement it, and only cascade-release its captured
        // values + free its block when *that* hits zero (the self-recursion cell
        // is a second block sharing the same env).
        self.builder.position_at_end(clos_free_bb);
        let epp = self.builder.build_struct_gep(self.closure_ty, cp, 2, "epp").unwrap();
        let cenv = self.builder.build_load(self.ptr_ty, epp, "cenv").unwrap().into_pointer_value();
        let cenv_addr = self.builder.build_ptr_to_int(cenv, i64t, "cenv_addr").unwrap();
        let cenv_null = self.builder.build_int_compare(EQ, cenv_addr, i64t.const_zero(), "cenv_null").unwrap();
        self.builder.build_conditional_branch(cenv_null, clos_block_free_bb, clos_env_live_bb).unwrap();

        self.builder.position_at_end(clos_env_live_bb);
        let ehdr = self.header_ptr(cenv);
        let ecur = self.builder.build_load(i64t, ehdr, "ecur").unwrap().into_int_value();
        let enext = self.builder.build_int_sub(ecur, i64t.const_int(1, false), "enext").unwrap();
        self.builder.build_store(ehdr, enext).unwrap();
        let ezero = self.builder.build_int_compare(EQ, enext, i64t.const_zero(), "ezero").unwrap();
        self.builder.build_conditional_branch(ezero, clos_env_free_bb, clos_block_free_bb).unwrap();

        self.builder.position_at_end(clos_env_free_bb);
        let en = self.builder.build_load(i64t, cenv, "en").unwrap().into_int_value();
        let eidxp = self.entry_alloca(i64t.into(), "erelidx");
        self.builder.build_store(eidxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(clos_env_loop_cond_bb).unwrap();

        self.builder.position_at_end(clos_env_loop_cond_bb);
        let ei = self.builder.build_load(i64t, eidxp, "ei").unwrap().into_int_value();
        let emore = self.builder.build_int_compare(SLT, ei, en, "emore").unwrap();
        self.builder.build_conditional_branch(emore, clos_env_loop_body_bb, clos_env_loop_end_bb).unwrap();

        self.builder.position_at_end(clos_env_loop_body_bb);
        let eslot = self.env_slot(cenv, ei);
        let ev = self.builder.build_load(self.value_ty, eslot, "ev").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[ev.into()]);
        let einext = self.builder.build_int_add(ei, i64t.const_int(1, false), "einext").unwrap();
        self.builder.build_store(eidxp, einext).unwrap();
        self.builder.build_unconditional_branch(clos_env_loop_cond_bb).unwrap();

        self.builder.position_at_end(clos_env_loop_end_bb);
        self.dec_live_counter();
        self.call_named("free", &[ehdr.into()]);
        self.builder.build_unconditional_branch(clos_block_free_bb).unwrap();

        self.builder.position_at_end(clos_block_free_bb);
        self.dec_live_counter();
        self.call_named("free", &[chdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- array: cascade into every element, then free elems + header -----
        self.builder.position_at_end(arr_check_bb);
        let is_arr = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ARRAY, false), "is_arr").unwrap();
        self.builder.build_conditional_branch(is_arr, arr_bb, map_check_bb).unwrap();

        self.builder.position_at_end(arr_bb);
        let ap = self.builder.build_int_to_ptr(p, self.ptr_ty, "ap").unwrap();
        let ahdr = self.header_ptr(ap);
        let acur = self.builder.build_load(i64t, ahdr, "acur").unwrap().into_int_value();
        let anext = self.builder.build_int_sub(acur, i64t.const_int(1, false), "anext").unwrap();
        self.builder.build_store(ahdr, anext).unwrap();
        let azero = self.builder.build_int_compare(EQ, anext, i64t.const_zero(), "azero").unwrap();
        self.builder.build_conditional_branch(azero, arr_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(arr_dec_bb);
        self.builder.build_unconditional_branch(arr_free_bb).unwrap();

        self.builder.position_at_end(arr_free_bb);
        let lenp = self.builder.build_struct_gep(self.array_ty, ap, 0, "lenp").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, ap, 2, "elemsp").unwrap();
        let len = self.builder.build_load(i64t, lenp, "len").unwrap().into_int_value();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let idxp = self.entry_alloca(i64t.into(), "relidx");
        self.builder.build_store(idxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(arr_loop_cond_bb).unwrap();

        self.builder.position_at_end(arr_loop_cond_bb);
        let i = self.builder.build_load(i64t, idxp, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(
            inkwell::IntPredicate::SLT, i, len, "more").unwrap();
        self.builder.build_conditional_branch(more, arr_loop_body_bb, arr_loop_end_bb).unwrap();

        self.builder.position_at_end(arr_loop_body_bb);
        let slot = unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot")
        }.unwrap();
        let elemv = self.builder.build_load(self.value_ty, slot, "elemv").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[elemv.into()]);
        let inext = self.builder.build_int_add(i, i64t.const_int(1, false), "inext").unwrap();
        self.builder.build_store(idxp, inext).unwrap();
        self.builder.build_unconditional_branch(arr_loop_cond_bb).unwrap();

        self.builder.position_at_end(arr_loop_end_bb);
        self.dec_live_counter();
        // `elems` (when non-null) was itself obtained via `malloc_bytes`/
        // `verb_alloc`, so -- like every other heap-owned pointer here --
        // its *actual* malloc'd address is `header_ptr(elems)` (elems-8),
        // not `elems` itself; freeing `elems` directly corrupts the heap.
        // A zero-length array's `elems` is a plain null (never routed
        // through `verb_alloc` at all, see `Expr::ArrayLit`), so guard on
        // that before computing/freeing the header.
        let elems_addr = self.builder.build_ptr_to_int(elems, i64t, "elems_addr").unwrap();
        let elems_null = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, elems_addr, i64t.const_zero(), "elems_null").unwrap();
        self.builder.build_conditional_branch(elems_null, arr_skip_elems_bb, arr_free_elems_bb).unwrap();

        self.builder.position_at_end(arr_free_elems_bb);
        // A non-empty array holds *two* separate verb_alloc blocks (the
        // header and this elems buffer), so freeing both needs two
        // decrements to balance the two increments `Expr::ArrayLit`
        // caused -- the single decrement above (paired with `ahdr`'s
        // free below) only accounts for the header.
        self.dec_live_counter();
        let ehdr = self.header_ptr(elems);
        self.call_named("free", &[ehdr.into()]);
        self.builder.build_unconditional_branch(arr_skip_elems_bb).unwrap();

        self.builder.position_at_end(arr_skip_elems_bb);
        self.call_named("free", &[ahdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- map: cascade via runtime/verb_map.cpp, then free header -----
        self.builder.position_at_end(map_check_bb);
        let is_map = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_MAP, false), "is_map").unwrap();
        self.builder.build_conditional_branch(is_map, map_bb, struct_check_bb).unwrap();

        self.builder.position_at_end(map_bb);
        let mp = self.builder.build_int_to_ptr(p, self.ptr_ty, "mp").unwrap();
        let mhdr = self.header_ptr(mp);
        let mcur = self.builder.build_load(i64t, mhdr, "mcur").unwrap().into_int_value();
        let mnext = self.builder.build_int_sub(mcur, i64t.const_int(1, false), "mnext").unwrap();
        self.builder.build_store(mhdr, mnext).unwrap();
        let mzero = self.builder.build_int_compare(EQ, mnext, i64t.const_zero(), "mzero").unwrap();
        self.builder.build_conditional_branch(mzero, map_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(map_dec_bb);
        self.builder.build_unconditional_branch(map_free_bb).unwrap();

        self.builder.position_at_end(map_free_bb);
        self.call_named("verb_map_destroy_contents", &[mp.into()]);
        self.dec_live_counter();
        self.call_named("free", &[mhdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- struct/record: cascade into every field, then free the one block -----
        self.builder.position_at_end(struct_check_bb);
        let is_struct = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STRUCT, false), "is_struct").unwrap();
        self.builder.build_conditional_branch(is_struct, struct_bb, enum_check_bb).unwrap();

        self.builder.position_at_end(struct_bb);
        let ssp = self.builder.build_int_to_ptr(p, self.ptr_ty, "ssp").unwrap();
        let shdr2 = self.header_ptr(ssp);
        let scur2 = self.builder.build_load(i64t, shdr2, "scur2").unwrap().into_int_value();
        let snext2 = self.builder.build_int_sub(scur2, i64t.const_int(1, false), "snext2").unwrap();
        self.builder.build_store(shdr2, snext2).unwrap();
        let szero2 = self.builder.build_int_compare(EQ, snext2, i64t.const_zero(), "szero2").unwrap();
        self.builder.build_conditional_branch(szero2, struct_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(struct_dec_bb);
        self.builder.build_unconditional_branch(struct_free_bb).unwrap();

        self.builder.position_at_end(struct_free_bb);
        let snfp = self.builder.build_struct_gep(self.struct_hdr_ty, ssp, 1, "snfp").unwrap();
        let snf = self.builder.build_load(i64t, snfp, "snf").unwrap().into_int_value();
        let sidxp = self.entry_alloca(i64t.into(), "srelidx");
        self.builder.build_store(sidxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(struct_loop_cond_bb).unwrap();

        self.builder.position_at_end(struct_loop_cond_bb);
        let si = self.builder.build_load(i64t, sidxp, "si").unwrap().into_int_value();
        let smore = self.builder.build_int_compare(
            inkwell::IntPredicate::SLT, si, snf, "smore").unwrap();
        self.builder.build_conditional_branch(smore, struct_loop_body_bb, struct_loop_end_bb).unwrap();

        self.builder.position_at_end(struct_loop_body_bb);
        let sslot = self.struct_field_slot(ssp, si);
        let sfv = self.builder.build_load(self.value_ty, sslot, "sfv").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[sfv.into()]);
        let sinext = self.builder.build_int_add(si, i64t.const_int(1, false), "sinext").unwrap();
        self.builder.build_store(sidxp, sinext).unwrap();
        self.builder.build_unconditional_branch(struct_loop_cond_bb).unwrap();

        self.builder.position_at_end(struct_loop_end_bb);
        // A struct is a single verb_alloc block (fields inline, descriptor is
        // a static global) -- one dec + one free, like a closure.
        self.dec_live_counter();
        self.call_named("free", &[shdr2.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- enum/variant: cascade into every field, then free the one block.
        // Identical to the struct cascade, but fields live after the 24-byte
        // enum header (descriptor + variant_id + nfields), and nfields is read
        // from enum_hdr slot 2. -----
        self.builder.position_at_end(enum_check_bb);
        let is_enum = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ENUM, false), "is_enum").unwrap();
        self.builder.build_conditional_branch(is_enum, enum_bb, done_bb).unwrap();

        self.builder.position_at_end(enum_bb);
        let esp = self.builder.build_int_to_ptr(p, self.ptr_ty, "esp").unwrap();
        let ehdr = self.header_ptr(esp);
        let ecur = self.builder.build_load(i64t, ehdr, "ecur").unwrap().into_int_value();
        let enext = self.builder.build_int_sub(ecur, i64t.const_int(1, false), "enext").unwrap();
        self.builder.build_store(ehdr, enext).unwrap();
        let ezero = self.builder.build_int_compare(EQ, enext, i64t.const_zero(), "ezero").unwrap();
        self.builder.build_conditional_branch(ezero, enum_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(enum_dec_bb);
        self.builder.build_unconditional_branch(enum_free_bb).unwrap();

        self.builder.position_at_end(enum_free_bb);
        let enfp = self.builder.build_struct_gep(self.enum_hdr_ty, esp, 2, "enfp").unwrap();
        let enf = self.builder.build_load(i64t, enfp, "enf").unwrap().into_int_value();
        let eidxp = self.entry_alloca(i64t.into(), "erelidx2");
        self.builder.build_store(eidxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(enum_loop_cond_bb).unwrap();

        self.builder.position_at_end(enum_loop_cond_bb);
        let ei = self.builder.build_load(i64t, eidxp, "ei").unwrap().into_int_value();
        let emore = self.builder.build_int_compare(SLT, ei, enf, "emore").unwrap();
        self.builder.build_conditional_branch(emore, enum_loop_body_bb, enum_loop_end_bb).unwrap();

        self.builder.position_at_end(enum_loop_body_bb);
        let eslot = self.enum_field_slot(esp, ei);
        let efv = self.builder.build_load(self.value_ty, eslot, "efv").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[efv.into()]);
        let einext = self.builder.build_int_add(ei, i64t.const_int(1, false), "einext").unwrap();
        self.builder.build_store(eidxp, einext).unwrap();
        self.builder.build_unconditional_branch(enum_loop_cond_bb).unwrap();

        self.builder.position_at_end(enum_loop_end_bb);
        self.dec_live_counter();
        self.call_named("free", &[ehdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }

    /// Runtime helper:
    /// verb_struct_find(value obj, ptr fname, i32 line, i32 col, ptr op) -> i64
    /// Aborts unless `obj` is a struct/record that has a field named
    /// `fname`; returns that field's index. `op` is a %s C string
    /// ("field access" / "field assignment") for the not-a-record message.
    /// Shared by verb_struct_get/verb_struct_set so field-name lookup lives
    /// in exactly one place.
    fn build_struct_find_fn(&self) {
        use inkwell::IntPredicate::{EQ, SLT};
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_struct_find",
            i64t.fn_type(
                &[self.value_ty.into(), self.ptr_ty.into(), i32t.into(), i32t.into(), self.ptr_ty.into()],
                false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let obj = f.get_nth_param(0).unwrap().into_struct_value();
        let fname = f.get_nth_param(1).unwrap().into_pointer_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let op = f.get_nth_param(4).unwrap().into_pointer_value();

        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let notstruct_bb = self.ctx.append_basic_block(f, "notstruct");
        let cond_bb = self.ctx.append_basic_block(f, "cond");
        let body_bb = self.ctx.append_basic_block(f, "body");
        let found_bb = self.ctx.append_basic_block(f, "found");
        let next_bb = self.ctx.append_basic_block(f, "next");
        let notfound_bb = self.ctx.append_basic_block(f, "notfound");

        let tag = self.tag_of(obj);
        let is_struct = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_STRUCT, false), "isstruct").unwrap();
        self.builder.build_conditional_branch(is_struct, ok_bb, notstruct_bb).unwrap();

        self.builder.position_at_end(notstruct_bb);
        self.abort_at(line, col, "'%s' needs a record, got %s", &[op.into(), self.type_name(tag)]);

        self.builder.position_at_end(ok_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(obj), self.ptr_ty, "sp").unwrap();
        let descp = self.builder.build_struct_gep(self.struct_hdr_ty, sp, 0, "descp").unwrap();
        let desc = self.builder.build_load(self.ptr_ty, descp, "desc").unwrap().into_pointer_value();
        let nfp = self.builder.build_struct_gep(self.struct_hdr_ty, sp, 1, "nfp").unwrap();
        let nf = self.builder.build_load(i64t, nfp, "nf").unwrap().into_int_value();
        let idxp = self.entry_alloca(i64t.into(), "fidx");
        self.builder.build_store(idxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(cond_bb);
        let i = self.builder.build_load(i64t, idxp, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(SLT, i, nf, "more").unwrap();
        self.builder.build_conditional_branch(more, body_bb, notfound_bb).unwrap();

        self.builder.position_at_end(body_bb);
        // descriptor names array starts at desc + 16, ptr-sized elements.
        let noff = self.builder.build_int_mul(i, i64t.const_int(8, false), "noff").unwrap();
        let noff = self.builder.build_int_add(noff, i64t.const_int(16, false), "noff16").unwrap();
        let namep_addr = unsafe {
            self.builder.build_in_bounds_gep(self.ctx.i8_type(), desc, &[noff], "namep_addr")
        }.unwrap();
        let name_i = self.builder.build_load(self.ptr_ty, namep_addr, "name_i").unwrap().into_pointer_value();
        let c = self.call_named("strcmp", &[name_i.into(), fname.into()]).unwrap().into_int_value();
        let eq = self.builder.build_int_compare(EQ, c, i32t.const_zero(), "nameeq").unwrap();
        self.builder.build_conditional_branch(eq, found_bb, next_bb).unwrap();

        self.builder.position_at_end(found_bb);
        self.builder.build_return(Some(&i)).unwrap();

        self.builder.position_at_end(next_bb);
        let inext = self.builder.build_int_add(i, i64t.const_int(1, false), "inext").unwrap();
        self.builder.build_store(idxp, inext).unwrap();
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(notfound_bb);
        self.abort_at(line, col, "unknown field '%s'", &[fname.into()]);
    }

    /// Runtime helper: verb_struct_get(value obj, ptr fname, i32, i32) -> value.
    /// Loads field `fname` from record `obj`, retaining the value handed
    /// back (mirrors Expr::Var / verb_array_get).
    fn build_struct_get_fn(&self) {
        self.build_struct_find_fn();
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_struct_get",
            self.value_ty.fn_type(&[self.value_ty.into(), self.ptr_ty.into(), i32t.into(), i32t.into()], false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let obj = f.get_nth_param(0).unwrap().into_struct_value();
        let fname = f.get_nth_param(1).unwrap().into_pointer_value();
        let line = f.get_nth_param(2).unwrap().into_int_value();
        let col = f.get_nth_param(3).unwrap().into_int_value();
        let op = self.cstr("field access");
        let i = self.call_named("verb_struct_find",
            &[obj.into(), fname.into(), line.into(), col.into(), op.into()])
            .unwrap().into_int_value();
        let sp = self.builder.build_int_to_ptr(self.payload_of(obj), self.ptr_ty, "sp").unwrap();
        let slot = self.struct_field_slot(sp, i);
        let v = self.builder.build_load(self.value_ty, slot, "v").unwrap().into_struct_value();
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_return(Some(&v)).unwrap();
    }

    /// Runtime helper: verb_struct_set(value obj, ptr fname, value v, i32, i32) -> value.
    /// Releases the field's previous value, stores `v` and retains it (the
    /// slot's new home), and returns `v` -- the array-set discipline.
    fn build_struct_set_fn(&self) {
        let i32t = self.ctx.i32_type();
        let f = self.module.add_function(
            "verb_struct_set",
            self.value_ty.fn_type(
                &[self.value_ty.into(), self.ptr_ty.into(), self.value_ty.into(), i32t.into(), i32t.into()],
                false),
            None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let obj = f.get_nth_param(0).unwrap().into_struct_value();
        let fname = f.get_nth_param(1).unwrap().into_pointer_value();
        let v = f.get_nth_param(2).unwrap().into_struct_value();
        let line = f.get_nth_param(3).unwrap().into_int_value();
        let col = f.get_nth_param(4).unwrap().into_int_value();
        let op = self.cstr("field assignment");
        let i = self.call_named("verb_struct_find",
            &[obj.into(), fname.into(), line.into(), col.into(), op.into()])
            .unwrap().into_int_value();
        let sp = self.builder.build_int_to_ptr(self.payload_of(obj), self.ptr_ty, "sp").unwrap();
        let slot = self.struct_field_slot(sp, i);
        let old = self.builder.build_load(self.value_ty, slot, "old").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[old.into()]);
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_store(slot, v).unwrap();
        self.builder.build_return(Some(&v)).unwrap();
    }

    /// Runtime helper: verb_retain_cell(ptr cell) -> void. Cells are
    /// always heap-owned (never static like a string literal can be), so
    /// this always bumps the header at cell-8, no tag/sentinel check.
    fn build_retain_cell_fn(&self) {
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.ptr_ty.into()], false);
        let f = self.module.add_function("verb_retain_cell", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let cell = f.get_nth_param(0).unwrap().into_pointer_value();
        let hdr = self.header_ptr(cell);
        let cur = self.builder.build_load(i64t, hdr, "cur").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i64t.const_int(1, false), "next").unwrap();
        self.builder.build_store(hdr, next).unwrap();
        self.builder.build_return(None).unwrap();
    }

    /// Runtime helper: verb_release_cell(ptr cell) -> void. Decrements
    /// the header at cell-8; at zero, releases the `VerbValue` stored
    /// inside (cascading into a heap-owned string/closure/array/map if
    /// that's what the cell holds) and frees the cell block itself.
    fn build_release_cell_fn(&self) {
        use inkwell::IntPredicate::*;
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.ptr_ty.into()], false);
        let f = self.module.add_function("verb_release_cell", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let free_bb = self.ctx.append_basic_block(f, "free");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let cell = f.get_nth_param(0).unwrap().into_pointer_value();
        let hdr = self.header_ptr(cell);
        let cur = self.builder.build_load(i64t, hdr, "cur").unwrap().into_int_value();
        let next = self.builder.build_int_sub(cur, i64t.const_int(1, false), "next").unwrap();
        self.builder.build_store(hdr, next).unwrap();
        let zero = self.builder.build_int_compare(EQ, next, i64t.const_zero(), "zero").unwrap();
        self.builder.build_conditional_branch(zero, free_bb, done_bb).unwrap();

        self.builder.position_at_end(free_bb);
        let inner = self.builder.build_load(self.value_ty, cell, "inner").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[inner.into()]);
        self.dec_live_counter();
        self.call_named("free", &[hdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }

    /// Heap-allocate a closure struct { fn_ptr, arity, env } and wrap it as a
    /// tagged value. `env` is the capture block (a `verb_alloc` block laid out
    /// `{ i64 n_captures, [n x value] }`) that the fn's body reads its captured
    /// variables from, or a null pointer for a non-capturing fn. The env's
    /// refcount is tracked at the granularity of *closure blocks* that point at
    /// it: this closure consumes one reference to `env`, so a caller that wants
    /// a *second* closure block over the same `env` (the self-recursion cell,
    /// see `Stmt::Fn`) must bump the env header itself.
    fn make_closure(&self, fnv: FunctionValue<'ctx>, arity: usize, env: PointerValue<'ctx>) -> StructValue<'ctx> {
        let p = self.malloc_bytes(24);
        let fp = fnv.as_global_value().as_pointer_value();
        self.builder.build_store(p, fp).unwrap();
        let ap = self.builder.build_struct_gep(self.closure_ty, p, 1, "ap").unwrap();
        self.builder.build_store(ap, self.ctx.i64_type().const_int(arity as u64, false)).unwrap();
        let ep = self.builder.build_struct_gep(self.closure_ty, p, 2, "ep").unwrap();
        self.builder.build_store(ep, env).unwrap();
        let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "cbits").unwrap();
        self.make_val(TAG_CLOSURE, bits)
    }

    /// Pointer to capture slot `idx` (a `value_ty`) inside an env block whose
    /// payload pointer is `env`. Captures live inline right after the 8-byte
    /// `{ i64 n_captures }` header, so slot `idx` is at `env + 8 + idx*16`.
    fn env_slot(&self, env: PointerValue<'ctx>, idx: IntValue<'ctx>) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let base = unsafe {
            self.builder.build_in_bounds_gep(self.ctx.i8_type(), env, &[i64t.const_int(8, false)], "ebase")
        }.unwrap();
        unsafe { self.builder.build_in_bounds_gep(self.value_ty, base, &[idx], "eslot") }.unwrap()
    }

    /// Bump the refcount header of a capture block, unless it is null (a
    /// non-capturing closure). Used when a *second* closure block starts to
    /// share an existing env (the self-recursion cell).
    fn retain_env(&self, env: PointerValue<'ctx>) {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let bump_bb = self.ctx.append_basic_block(f, "env.retain.bump");
        let done_bb = self.ctx.append_basic_block(f, "env.retain.done");
        let addr = self.builder.build_ptr_to_int(env, i64t, "envaddr").unwrap();
        let is_null = self.builder.build_int_compare(EQ, addr, i64t.const_zero(), "envnull").unwrap();
        self.builder.build_conditional_branch(is_null, done_bb, bump_bb).unwrap();
        self.builder.position_at_end(bump_bb);
        let hdr = self.header_ptr(env);
        let cur = self.builder.build_load(i64t, hdr, "envcur").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i64t.const_int(1, false), "envnext").unwrap();
        self.builder.build_store(hdr, next).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();
        self.builder.position_at_end(done_bb);
    }

    /// Alloca in the current function's entry block so loops don't grow the stack.
    fn entry_alloca(&self, ty: inkwell::types::BasicTypeEnum<'ctx>, name: &str)
        -> PointerValue<'ctx>
    {
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let entry = f.get_first_basic_block().unwrap();
        let tmp = self.ctx.create_builder();
        match entry.get_first_instruction() {
            Some(i) => tmp.position_before(&i),
            None => tmp.position_at_end(entry),
        }
        tmp.build_alloca(ty, name).unwrap()
    }

    // ----- program -----

    pub fn compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String], imports: &[String], std_imports: &[String]) -> Result<(), CompileError> {
        self.imports = imports.to_vec();
        self.std_imports = std_imports.to_vec();
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        for (i, s) in stmts.iter().enumerate() {
            self.cur_file = stmt_files[i].clone();
            if let Err(mut e) = self.gen_stmt(s) {
                if e.file.is_none() {
                    e.file = Some(self.cur_file.clone());
                }
                return Err(e);
            }
            if !self.cur_block_open() {
                break; // dead code after return/abort
            }
        }
        if self.cur_block_open() {
            for slot in self.globals.values() {
                let v = self.builder.build_load(self.value_ty, *slot, "gval").unwrap().into_struct_value();
                self.call_named("verb_release_value", &[v.into()]);
            }
            self.emit_gc_debug_dump(main);
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }

    fn cur_block_open(&self) -> bool {
        self.builder.get_insert_block().unwrap().get_terminator().is_none()
    }

    fn gen_stmts(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        for s in stmts {
            self.gen_stmt(s)?;
            if !self.cur_block_open() { break; } // dead code after return/abort
        }
        Ok(())
    }

    /// Own params/locals (and those of enclosing blocks in the *same* function),
    /// falling back to top-level globals. A nested `make` never sees an
    /// enclosing function's scope — its `self.scopes` is reset to empty before
    /// its body is compiled (see `Stmt::Fn`), so this can only ever reach back
    /// as far as the function's own frames, then straight to `globals`.
    fn lookup(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
            .or_else(|| self.globals.get(name).copied())
    }

    /// Address of the module-level global variable backing top-level name
    /// `name`, creating it on first use. Unlike a heap cell from
    /// `malloc_bytes` (whose pointer is an SSA value scoped to the function
    /// that computed it), a global variable's address is a module-wide
    /// constant valid in every function's IR — required for a nested `make`
    /// to read/write it.
    fn global_slot(&mut self, name: &str) -> PointerValue<'ctx> {
        if let Some(&p) = self.globals.get(name) {
            return p;
        }
        let g = self.module.add_global(self.value_ty, None, &format!("g.{name}"));
        g.set_initializer(&self.value_ty.const_zero());
        let p = g.as_pointer_value();
        self.globals.insert(name.to_string(), p);
        p
    }

    /// Bind in the innermost active scope, or as a top-level global when not
    /// inside any function/block (`self.scopes` is empty).
    fn bind(&mut self, name: &str, value: StructValue<'ctx>) {
        if self.scopes.is_empty() {
            let slot = self.global_slot(name);
            let old = self.builder.build_load(self.value_ty, slot, "old_global").unwrap().into_struct_value();
            self.call_named("verb_release_value", &[old.into()]);
            self.builder.build_store(slot, value).unwrap();
        } else {
            let cell = self.malloc_bytes(16);
            self.builder.build_store(cell, value).unwrap();
            if let Some(old_cell) = self.scopes.last_mut().unwrap().insert(name.to_string(), cell) {
                self.call_named("verb_release_cell", &[old_cell.into()]);
            }
        }
    }

    /// Hint for an unresolved name: keyword rename, else closest known name.
    fn name_hint(&self, name: &str) -> Option<String> {
        if let Some(new) = crate::lexer::renamed_keyword(name) {
            return Some(format!("'{name}' was renamed to '{new}'"));
        }
        let best = self.scopes.iter().flat_map(|s| s.keys())
            .chain(self.globals.keys())
            .map(|cand| (levenshtein(name, cand), cand))
            .min()?;
        (best.0 <= 2 && best.0 < name.len())
            .then(|| format!("did you mean '{}'?", best.1))
    }

    fn undefined_var(&self, name: &str, line: u32, col: u32) -> CompileError {
        let e = CompileError::new(format!("undefined variable '{name}'"), line, col);
        match self.name_hint(name) {
            Some(h) => e.with_hint(h),
            None => e,
        }
    }

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::ExprStmt(e) => {
                let v = self.gen_expr(e)?;
                self.call_named("verb_release_value", &[v.into()]);
                Ok(())
            }
            Stmt::Assign { name, value } => {
                let v = self.gen_expr(value)?;
                self.bind(name, v);
                Ok(())
            }
            Stmt::Declare { name } => {
                let nil = self.nil_val();
                self.bind(name, nil);
                Ok(())
            }
            Stmt::Reassign { name, value, line, col } => {
                let cell = self.lookup(name).ok_or_else(|| {
                    self.undefined_var(name, *line, *col)
                        .with_hint("declare new variables with 'assign' or 'declare'".to_string())
                })?;
                let v = self.gen_expr(value)?;
                let old = self.builder.build_load(self.value_ty, cell, "old").unwrap().into_struct_value();
                self.call_named("verb_release_value", &[old.into()]);
                self.builder.build_store(cell, v).unwrap();
                Ok(())
            }
            Stmt::Block(stmts) => {
                self.scopes.push(HashMap::new());
                let r = self.gen_stmts(stmts);
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                r
            }
            Stmt::If { cond, then_body, else_body } => {
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.call_named("verb_release_value", &[cv.into()]);
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let then_bb = self.ctx.append_basic_block(f, "if.then");
                let else_bb = self.ctx.append_basic_block(f, "if.else");
                let merge = self.ctx.append_basic_block(f, "if.end");
                self.builder.build_conditional_branch(t, then_bb, else_bb).unwrap();

                self.builder.position_at_end(then_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(then_body)?;
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }

                self.builder.position_at_end(else_bb);
                if let Some(eb) = else_body {
                    self.scopes.push(HashMap::new());
                    self.gen_stmts(eb)?;
                    if self.cur_block_open() {
                        if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                    } else {
                        self.scopes.pop();
                    }
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }
                self.builder.position_at_end(merge);
                Ok(())
            }
            Stmt::While { cond, body } => {
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let cond_bb = self.ctx.append_basic_block(f, "while.cond");
                let body_bb = self.ctx.append_basic_block(f, "while.body");
                let end_bb = self.ctx.append_basic_block(f, "while.end");
                self.builder.build_unconditional_branch(cond_bb).unwrap();

                self.builder.position_at_end(cond_bb);
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.call_named("verb_release_value", &[cv.into()]);
                self.builder.build_conditional_branch(t, body_bb, end_bb).unwrap();

                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
                self.builder.position_at_end(end_bb);
                Ok(())
            }
            Stmt::Fn { name, params, body, .. } => {
                self.fn_counter += 1;
                let llname = format!("fn.{}.{}", name, self.fn_counter);
                let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
                let fnv = self.module.add_function(&llname, fnty, None);

                // Free-variable analysis (must run *before* `mem::take` below,
                // while `self.scopes` still holds the enclosing function's
                // frames). A candidate is captured -- by value, a snapshot --
                // only if it resolves to an enclosing *local*; names that
                // resolve to a global (or not at all: record types, builtins)
                // stay as they were and are ignored here.
                let captures: Vec<(String, PointerValue<'ctx>)> = free_vars(name, params, body)
                    .into_iter()
                    .filter_map(|n| {
                        let cell = self.scopes.iter().rev().find_map(|s| s.get(&n).copied())?;
                        Some((n, cell))
                    })
                    .collect();

                // Build the capture env in the enclosing frame (where the
                // captured cells are live) and snapshot each captured value into
                // it (retained: the env now co-owns them). Empty capture -> null
                // env, leaving non-capturing fns byte-for-byte as before.
                let i64t = self.ctx.i64_type();
                let env = if captures.is_empty() {
                    self.ptr_ty.const_null()
                } else {
                    let bytes = 8 + captures.len() as u64 * 16;
                    let e = self.malloc_bytes(bytes);
                    self.builder.build_store(e, i64t.const_int(captures.len() as u64, false)).unwrap();
                    for (i, (_, cell)) in captures.iter().enumerate() {
                        let v = self.builder.build_load(self.value_ty, *cell, "capv").unwrap().into_struct_value();
                        self.call_named("verb_retain_value", &[v.into()]);
                        let slot = self.env_slot(e, i64t.const_int(i as u64, false));
                        self.builder.build_store(slot, v).unwrap();
                    }
                    e
                };
                let capture_names: Vec<String> = captures.iter().map(|(n, _)| n.clone()).collect();

                // bind the name as a first-class closure value in the enclosing
                // scope (globals, if at top level) so callers can reference it.
                // This closure block consumes the env's initial reference.
                let clos = self.make_closure(fnv, params.len(), env);
                self.bind(name, clos);

                let saved_bb = self.builder.get_insert_block().unwrap();
                // A nested `make` sees only its own params/locals and top-level
                // globals -- never the enclosing function's scope, not even
                // names declared before it. Wiping `scopes` (rather than just
                // pushing a new frame) enforces that: `lookup` can only walk
                // back to frames pushed for *this* function, then falls through
                // to `globals`.
                let saved_scopes = std::mem::take(&mut self.scopes);
                self.fn_depth += 1;

                let entry = self.ctx.append_basic_block(fnv, "entry");
                self.builder.position_at_end(entry);
                // own name, bound locally too, so self-recursion resolves
                // without leaking the name into the enclosing/global scope.
                // Must be built fresh here (not the closure bound above) since
                // a malloc'd heap cell's pointer is an SSA value scoped to the
                // function that computed it -- the outer function's, in this
                // case -- and can't be reused inside this new function's IR.
                // Self-recursion closure: a *separate* closure block sharing
                // this invocation's env (arg 0), so a recursive call re-supplies
                // the same captures. It's a second block over `env`, so bump the
                // env header (balanced when this cell is released at fn exit).
                let body_env = fnv.get_nth_param(0).unwrap().into_pointer_value();
                let self_clos = self.make_closure(fnv, params.len(), body_env);
                self.retain_env(body_env);
                let self_cell = self.malloc_bytes(16);
                self.builder.build_store(self_cell, self_clos).unwrap();
                let argv = fnv.get_nth_param(1).unwrap().into_pointer_value();
                let mut scope = HashMap::new();
                scope.insert(name.clone(), self_cell);
                // Re-materialize each captured variable as an ordinary local
                // cell, loaded from the env block (slot order fixed by
                // `capture_names`) and retained. `lookup` then resolves captures
                // exactly like params -- which is why the `mem::take` above can
                // stay: the enclosing frame is never consulted.
                for (i, cn) in capture_names.iter().enumerate() {
                    let slot = self.env_slot(body_env, i64t.const_int(i as u64, false));
                    let v = self.builder.build_load(self.value_ty, slot, cn).unwrap().into_struct_value();
                    self.call_named("verb_retain_value", &[v.into()]);
                    let cell = self.malloc_bytes(16);
                    self.builder.build_store(cell, v).unwrap();
                    scope.insert(cn.clone(), cell);
                }
                for (i, p) in params.iter().enumerate() {
                    let ap = unsafe {
                        self.builder.build_in_bounds_gep(
                            self.value_ty, argv,
                            &[self.ctx.i64_type().const_int(i as u64, false)], p)
                    }.unwrap();
                    let v = self.builder.build_load(self.value_ty, ap, p).unwrap();
                    let cell = self.malloc_bytes(16);
                    self.builder.build_store(cell, v).unwrap();
                    scope.insert(p.clone(), cell);
                }
                self.scopes.push(scope);
                let r = self.gen_stmts(body);
                if self.cur_block_open() {
                    self.release_all_open_scopes();
                    self.builder.build_return(Some(&self.nil_val())).unwrap();
                }
                self.scopes.pop();

                self.fn_depth -= 1;
                self.scopes = saved_scopes;
                self.builder.position_at_end(saved_bb);
                r
            }
            Stmt::Return { value } => {
                if self.fn_depth == 0 {
                    return Err(CompileError::new("'return' outside function", 0, 0));
                }
                let v = match value {
                    Some(e) => self.gen_expr(e)?,
                    None => self.nil_val(),
                };
                self.release_all_open_scopes();
                self.builder.build_return(Some(&v)).unwrap();
                Ok(())
            }
            Stmt::Record { name, fields, .. } => {
                self.gen_record(name, fields);
                Ok(())
            }
            Stmt::Choice { name, variants, .. } => {
                self.gen_choice(name, variants);
                Ok(())
            }
            Stmt::Match { scrutinee, arms, otherwise, line, col } => {
                self.gen_match(scrutinee, arms, otherwise.as_deref(), *line, *col)
            }
            Stmt::FieldSet { obj, field, value, line, col } => {
                let o = self.gen_expr(obj)?;
                let val = self.gen_expr(value)?;
                let fname = self.cstr(field);
                let (lc, cc) = self.loc_consts(*line, *col);
                let rv = self.call_named(
                    "verb_struct_set", &[o.into(), fname.into(), val.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                // Release the struct temp and the returned value temp (which
                // is `val`, retained once inside verb_struct_set for its new
                // home in the field slot).
                self.call_named("verb_release_value", &[o.into()]);
                self.call_named("verb_release_value", &[rv.into()]);
                Ok(())
            }
        }
    }

    /// Builds a record's static descriptor global
    /// ({ i8* type_name, i64 nfields, [nfields x i8*] field_names }) and
    /// registers it in `self.records`. Emits no runtime instructions.
    fn gen_record(&mut self, name: &str, fields: &[String]) {
        let i64t = self.ctx.i64_type();
        let n = fields.len() as u64;
        let name_const = self.cstr(name);
        let field_consts: Vec<PointerValue<'ctx>> = fields.iter().map(|fld| self.cstr(fld)).collect();
        let names_arr = self.ptr_ty.const_array(&field_consts);
        let desc_ty = self.ctx.struct_type(
            &[self.ptr_ty.into(), i64t.into(), self.ptr_ty.array_type(n as u32).into()], false);
        let init = desc_ty.const_named_struct(&[
            name_const.into(),
            i64t.const_int(n, false).into(),
            names_arr.into(),
        ]);
        let g = self.module.add_global(desc_ty, None, &format!("verb.rec.{name}"));
        g.set_initializer(&init);
        g.set_constant(true);
        self.records.insert(name.to_string(), RecordInfo {
            descriptor: g.as_pointer_value(),
            fields: fields.to_vec(),
        });
    }

    /// Builds one static descriptor global per variant of a `choice` -- each
    /// shaped exactly like a record descriptor ({ i8* variant_name, i64
    /// nfields, [nfields x i8*] field_names }) -- and registers every variant
    /// (keyed by its own name, flat across choices) in `self.variants` with
    /// its index as `variant_id`. Emits no runtime instructions.
    fn gen_choice(&mut self, name: &str, variants: &[(String, Vec<String>)]) {
        let i64t = self.ctx.i64_type();
        for (vid, (vname, fields)) in variants.iter().enumerate() {
            let n = fields.len() as u64;
            let name_const = self.cstr(vname);
            let field_consts: Vec<PointerValue<'ctx>> =
                fields.iter().map(|fld| self.cstr(fld)).collect();
            let names_arr = self.ptr_ty.const_array(&field_consts);
            let desc_ty = self.ctx.struct_type(
                &[self.ptr_ty.into(), i64t.into(), self.ptr_ty.array_type(n as u32).into()], false);
            let init = desc_ty.const_named_struct(&[
                name_const.into(),
                i64t.const_int(n, false).into(),
                names_arr.into(),
            ]);
            let g = self.module.add_global(desc_ty, None, &format!("verb.choice.{name}.{vname}"));
            g.set_initializer(&init);
            g.set_constant(true);
            self.variants.insert(vname.clone(), VariantInfo {
                descriptor: g.as_pointer_value(),
                variant_id: vid as u64,
                fields: fields.clone(),
            });
        }
    }

    /// Allocates and initializes an enum heap block for `info` with the given
    /// (already-owned) field values, returning the tagged value. Layout is
    /// `[ptr descriptor][i64 variant_id][i64 nfields][fields...]`, a single
    /// `verb_alloc` block (like a struct, plus the variant_id word).
    fn build_variant(&self, info: &VariantInfo<'ctx>, vals: Vec<StructValue<'ctx>>)
        -> StructValue<'ctx>
    {
        let i64t = self.ctx.i64_type();
        let n = vals.len() as u64;
        let sp = self.malloc_bytes(24 + n * 16);
        let descp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 0, "edescp").unwrap();
        self.builder.build_store(descp, info.descriptor).unwrap();
        let vidp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 1, "evidp").unwrap();
        self.builder.build_store(vidp, i64t.const_int(info.variant_id, false)).unwrap();
        let nfp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 2, "enfp").unwrap();
        self.builder.build_store(nfp, i64t.const_int(n, false)).unwrap();
        for (i, v) in vals.into_iter().enumerate() {
            let slot = self.enum_field_slot(sp, i64t.const_int(i as u64, false));
            self.builder.build_store(slot, v).unwrap();
        }
        let bits = self.builder.build_ptr_to_int(sp, i64t, "ebits").unwrap();
        self.make_val(TAG_ENUM, bits)
    }

    /// `is_err(v)` builtin -> a `bool` value, true iff `v` is an enum whose
    /// variant is the built-in `Err`. Consumes (releases) `v`.
    fn gen_is_err(&mut self, v: StructValue<'ctx>) -> StructValue<'ctx> {
        use inkwell::IntPredicate::EQ;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let err_id = self.variants.get("Err").expect("built-in Err variant").variant_id;
        let tag = self.tag_of(v);
        let is_enum = self.builder.build_int_compare(
            EQ, tag, i8t.const_int(TAG_ENUM, false), "iserr.isenum").unwrap();
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let entry_bb = self.builder.get_insert_block().unwrap();
        let load_bb = self.ctx.append_basic_block(f, "iserr.load");
        let merge_bb = self.ctx.append_basic_block(f, "iserr.merge");
        self.builder.build_conditional_branch(is_enum, load_bb, merge_bb).unwrap();

        self.builder.position_at_end(load_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(v), self.ptr_ty, "iserr.sp").unwrap();
        let vidp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 1, "iserr.vidp").unwrap();
        let vid = self.builder.build_load(i64t, vidp, "iserr.vid").unwrap().into_int_value();
        let eq = self.builder.build_int_compare(
            EQ, vid, i64t.const_int(err_id, false), "iserr.eq").unwrap();
        let load_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.ctx.bool_type(), "iserr.phi").unwrap();
        let false_c = self.ctx.bool_type().const_zero();
        phi.add_incoming(&[(&false_c, entry_bb), (&eq, load_end)]);
        let b = phi.as_basic_value().into_int_value();
        self.call_named("verb_release_value", &[v.into()]);
        self.bool_val(b)
    }

    /// Shared impl of `err_kind(v)` (idx 0) and `err_msg(v)` (idx 1): the
    /// requested field of `v` if `v` is a built-in `Err` enum, else `nil`.
    /// The returned field is retained (independently owned) and `v` is
    /// released -- the same ownership handoff `match` arm bindings use.
    fn gen_err_field(&mut self, v: StructValue<'ctx>, idx: u64) -> StructValue<'ctx> {
        use inkwell::IntPredicate::EQ;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let err_id = self.variants.get("Err").expect("built-in Err variant").variant_id;
        let tag = self.tag_of(v);
        let is_enum = self.builder.build_int_compare(
            EQ, tag, i8t.const_int(TAG_ENUM, false), "errf.isenum").unwrap();
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let chk_bb = self.ctx.append_basic_block(f, "errf.chk");
        let load_bb = self.ctx.append_basic_block(f, "errf.load");
        let nil_bb = self.ctx.append_basic_block(f, "errf.nil");
        let merge_bb = self.ctx.append_basic_block(f, "errf.merge");
        self.builder.build_conditional_branch(is_enum, chk_bb, nil_bb).unwrap();

        self.builder.position_at_end(chk_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(v), self.ptr_ty, "errf.sp").unwrap();
        let vidp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 1, "errf.vidp").unwrap();
        let vid = self.builder.build_load(i64t, vidp, "errf.vid").unwrap().into_int_value();
        let is_err = self.builder.build_int_compare(
            EQ, vid, i64t.const_int(err_id, false), "errf.iserr").unwrap();
        self.builder.build_conditional_branch(is_err, load_bb, nil_bb).unwrap();

        self.builder.position_at_end(load_bb);
        let slot = self.enum_field_slot(sp, i64t.const_int(idx, false));
        let fv = self.builder.build_load(self.value_ty, slot, "errf.fv").unwrap().into_struct_value();
        self.call_named("verb_retain_value", &[fv.into()]);
        let load_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(nil_bb);
        let nilv = self.nil_val();
        let nil_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.value_ty, "errf.phi").unwrap();
        phi.add_incoming(&[(&fv, load_end), (&nilv, nil_end)]);
        let result = phi.as_basic_value().into_struct_value();
        self.call_named("verb_release_value", &[v.into()]);
        result
    }

    /// A `value_ty` holding a static Verb string literal (mirrors `Expr::Str`
    /// codegen). Used to build `Err` payloads from generated code.
    fn string_value(&self, s: &str) -> StructValue<'ctx> {
        let p = self.static_string_ptr(s);
        let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "strbits").unwrap();
        self.make_val(TAG_STR, bits)
    }

    /// Wraps a std-io call `result` so a `nil` failure sentinel becomes a
    /// built-in `Err("io", "<name> failed")`; any non-nil (success) value
    /// passes through unchanged. This is where the std-io failure contract
    /// (nil -> Err) is realized: the C++ runtime still returns nil (it has no
    /// access to the enum descriptor globals), and codegen lifts it here.
    fn lift_nil_to_err(&mut self, result: StructValue<'ctx>, name: &str) -> StructValue<'ctx> {
        use inkwell::IntPredicate::EQ;
        let i8t = self.ctx.i8_type();
        let tag = self.tag_of(result);
        let is_nil = self.builder.build_int_compare(
            EQ, tag, i8t.const_int(TAG_NIL, false), "lift.isnil").unwrap();
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let entry_bb = self.builder.get_insert_block().unwrap();
        let err_bb = self.ctx.append_basic_block(f, "lift.err");
        let merge_bb = self.ctx.append_basic_block(f, "lift.merge");
        self.builder.build_conditional_branch(is_nil, err_bb, merge_bb).unwrap();

        self.builder.position_at_end(err_bb);
        let kind = self.string_value("io");
        let msg = self.string_value(&format!("{name} failed"));
        let info = self.variants.get("Err").expect("built-in Err variant").clone();
        let errv = self.build_variant(&info, vec![kind, msg]);
        let err_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.value_ty, "lift.phi").unwrap();
        phi.add_incoming(&[(&result, entry_bb), (&errv, err_end)]);
        phi.as_basic_value().into_struct_value()
    }

    /// `match` codegen: evaluate the scrutinee, then an if-chain over the arms
    /// keyed on the loaded variant_id. Each matching arm loads+retains its
    /// bound fields, releases the scrutinee temp, then binds the fields into a
    /// fresh scope and runs its body. A non-enum scrutinee, or an enum whose
    /// variant no arm covers, falls to `otherwise` (if present) or a runtime
    /// abort. Fields are retained *before* the scrutinee is released so an
    /// early `return` in a body leaks nothing (the temp is never held across
    /// the body).
    fn gen_match(&mut self, scrutinee: &Expr, arms: &[MatchArm],
                 otherwise: Option<&[Stmt]>, line: u32, col: u32)
        -> Result<(), CompileError>
    {
        use inkwell::IntPredicate::EQ;
        let i64t = self.ctx.i64_type();
        let i8t = self.ctx.i8_type();
        let sv = self.gen_expr(scrutinee)?;
        let tag = self.tag_of(sv);
        let is_enum = self.builder.build_int_compare(
            EQ, tag, i8t.const_int(TAG_ENUM, false), "match.isenum").unwrap();
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let dispatch_bb = self.ctx.append_basic_block(f, "match.dispatch");
        let nomatch_bb = self.ctx.append_basic_block(f, "match.nomatch");
        let merge_bb = self.ctx.append_basic_block(f, "match.end");
        self.builder.build_conditional_branch(is_enum, dispatch_bb, nomatch_bb).unwrap();

        self.builder.position_at_end(dispatch_bb);
        let sp = self.builder.build_int_to_ptr(self.payload_of(sv), self.ptr_ty, "match.sp").unwrap();
        let vidp = self.builder.build_struct_gep(self.enum_hdr_ty, sp, 1, "match.vidp").unwrap();
        let vid = self.builder.build_load(i64t, vidp, "match.vid").unwrap().into_int_value();

        let mut test_bbs = Vec::with_capacity(arms.len());
        let mut body_bbs = Vec::with_capacity(arms.len());
        for i in 0..arms.len() {
            test_bbs.push(self.ctx.append_basic_block(f, &format!("match.test{i}")));
            body_bbs.push(self.ctx.append_basic_block(f, &format!("match.body{i}")));
        }
        let first = test_bbs.first().copied().unwrap_or(nomatch_bb);
        self.builder.build_unconditional_branch(first).unwrap();

        for (i, arm) in arms.iter().enumerate() {
            let info = self.variants.get(&arm.variant).cloned().ok_or_else(|| {
                CompileError::new(format!("unknown variant '{}' in match", arm.variant), line, col)
            })?;
            if arm.bindings.len() != info.fields.len() {
                return Err(CompileError::new(
                    format!("variant '{}' has {} field(s), but the pattern binds {}",
                            arm.variant, info.fields.len(), arm.bindings.len()),
                    line, col));
            }
            self.builder.position_at_end(test_bbs[i]);
            let want = i64t.const_int(info.variant_id, false);
            let eq = self.builder.build_int_compare(EQ, vid, want, "match.eq").unwrap();
            let next = test_bbs.get(i + 1).copied().unwrap_or(nomatch_bb);
            self.builder.build_conditional_branch(eq, body_bbs[i], next).unwrap();

            self.builder.position_at_end(body_bbs[i]);
            // Load + retain every bound field BEFORE releasing the scrutinee.
            let mut bound_vals = Vec::with_capacity(arm.bindings.len());
            for j in 0..arm.bindings.len() {
                let slot = self.enum_field_slot(sp, i64t.const_int(j as u64, false));
                let fv = self.builder.build_load(self.value_ty, slot, "match.fv")
                    .unwrap().into_struct_value();
                self.call_named("verb_retain_value", &[fv.into()]);
                bound_vals.push(fv);
            }
            self.call_named("verb_release_value", &[sv.into()]);
            self.scopes.push(HashMap::new());
            for (bn, bv) in arm.bindings.iter().zip(bound_vals.into_iter()) {
                self.bind(bn, bv);
            }
            self.gen_stmts(&arm.body)?;
            if self.cur_block_open() {
                if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
            } else {
                self.scopes.pop();
            }
            if self.cur_block_open() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        self.builder.position_at_end(nomatch_bb);
        self.call_named("verb_release_value", &[sv.into()]);
        if let Some(ob) = otherwise {
            self.scopes.push(HashMap::new());
            self.gen_stmts(ob)?;
            if self.cur_block_open() {
                if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
            } else {
                self.scopes.pop();
            }
            if self.cur_block_open() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        } else {
            let (lc, cc) = self.loc_consts(line, col);
            self.abort_at(lc, cc, "no matching variant in match", &[]);
        }

        self.builder.position_at_end(merge_bb);
        Ok(())
    }

    fn gen_expr(&mut self, expr: &Expr) -> Result<StructValue<'ctx>, CompileError> {
        match expr {
            Expr::Int(v) => Ok(self.make_val(TAG_INT, self.ctx.i64_type().const_int(*v as u64, true))),
            Expr::Float(v) => {
                let bits = self.builder.build_bit_cast(
                    self.ctx.f64_type().const_float(*v), self.ctx.i64_type(), "bits",
                ).unwrap().into_int_value();
                Ok(self.make_val(TAG_FLOAT, bits))
            }
            Expr::Str(s) => {
                let p = self.static_string_ptr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
            Expr::Bool(b) => Ok(self.make_val(TAG_BOOL, self.ctx.i64_type().const_int(*b as u64, false))),
            Expr::Nil => Ok(self.nil_val()),
            Expr::Var(name, line, col) => {
                if let Some(cell) = self.lookup(name) {
                    let v = self.builder.build_load(self.value_ty, cell, name)
                        .unwrap().into_struct_value();
                    self.call_named("verb_retain_value", &[v.into()]);
                    return Ok(v);
                }
                // A bare name that is a fieldless enum variant constructs that
                // variant (a fielded variant must be written `V(..)`, handled
                // in gen_call).
                if let Some(info) = self.variants.get(name).cloned() {
                    if info.fields.is_empty() {
                        return Ok(self.build_variant(&info, Vec::new()));
                    }
                    return Err(CompileError::new(
                        format!("variant '{name}' takes {} field(s) -- construct it as {name}(..)",
                                info.fields.len()),
                        *line, *col));
                }
                Err(self.undefined_var(name, *line, *col))
            }
            Expr::Binary { op, lhs, rhs, line, col } => self.gen_binary(*op, lhs, rhs, *line, *col),
            Expr::Unary { op, expr, line, col } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let (lc, cc) = self.loc_consts(*line, *col);
                        let out = self.call_named("verb_neg", &[v.into(), lc.into(), cc.into()])
                            .unwrap().into_struct_value();
                        self.call_named("verb_release_value", &[v.into()]);
                        Ok(out)
                    }
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        self.call_named("verb_release_value", &[v.into()]);
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
            Expr::Call { callee, args, line, col } => self.gen_call(callee, args, *line, *col),
            Expr::ArrayLit(elems) => {
                let n = elems.len() as u64;
                let hdr = self.malloc_bytes(24); // { i64 len, i64 cap, ptr elems }
                let elems_buf = if n == 0 {
                    self.ptr_ty.const_null()
                } else {
                    self.malloc_bytes(n * 16) // n * sizeof(%verb.value)
                };
                for (i, e) in elems.iter().enumerate() {
                    let v = self.gen_expr(e)?;
                    let slot = unsafe {
                        self.builder.build_in_bounds_gep(
                            self.value_ty, elems_buf,
                            &[self.ctx.i64_type().const_int(i as u64, false)], "slot")
                    }.unwrap();
                    self.builder.build_store(slot, v).unwrap();
                }
                let lenp = self.builder.build_struct_gep(self.array_ty, hdr, 0, "lenp").unwrap();
                self.builder.build_store(lenp, self.ctx.i64_type().const_int(n, false)).unwrap();
                let capp = self.builder.build_struct_gep(self.array_ty, hdr, 1, "capp").unwrap();
                self.builder.build_store(capp, self.ctx.i64_type().const_int(n, false)).unwrap();
                let elemsp = self.builder.build_struct_gep(self.array_ty, hdr, 2, "elemsp").unwrap();
                self.builder.build_store(elemsp, elems_buf).unwrap();
                let bits = self.builder.build_ptr_to_int(hdr, self.ctx.i64_type(), "abits").unwrap();
                Ok(self.make_val(TAG_ARRAY, bits))
            }
            Expr::FieldGet { obj, field, line, col } => {
                let o = self.gen_expr(obj)?;
                let fname = self.cstr(field);
                let (lc, cc) = self.loc_consts(*line, *col);
                let rv = self.call_named(
                    "verb_struct_get", &[o.into(), fname.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[o.into()]);
                Ok(rv)
            }
        }
    }

    fn loc_consts(&self, line: u32, col: u32) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let i32t = self.ctx.i32_type();
        (i32t.const_int(line as u64, false), i32t.const_int(col as u64, false))
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        // short-circuit: 'and'/'or' return operand values (Lox semantics)
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.gen_expr(lhs)?;
            let t = self.call_named("verb_truthy", &[l.into()]).unwrap().into_int_value();
            let cur_fn = self.builder.get_insert_block().unwrap().get_parent().unwrap();
            let lhs_end = self.builder.get_insert_block().unwrap();
            let rhs_bb = self.ctx.append_basic_block(cur_fn, "sc.rhs");
            let merge = self.ctx.append_basic_block(cur_fn, "sc.end");
            match op {
                BinOp::And => self.builder.build_conditional_branch(t, rhs_bb, merge).unwrap(),
                _ => self.builder.build_conditional_branch(t, merge, rhs_bb).unwrap(),
            };
            self.builder.position_at_end(rhs_bb);
            let r = self.gen_expr(rhs)?;
            // rhs_bb is only entered when `r` becomes the result instead of
            // `l`, so the owned temporary `l` is being discarded here.
            self.call_named("verb_release_value", &[l.into()]);
            let rhs_end = self.builder.get_insert_block().unwrap();
            self.builder.build_unconditional_branch(merge).unwrap();
            self.builder.position_at_end(merge);
            let phi = self.builder.build_phi(self.value_ty, "sc").unwrap();
            phi.add_incoming(&[(&l, lhs_end), (&r, rhs_end)]);
            return Ok(phi.as_basic_value().into_struct_value());
        }

        let l = self.gen_expr(lhs)?;
        let r = self.gen_expr(rhs)?;
        let out = if matches!(op, BinOp::Eq | BinOp::Ne) {
            // eq never aborts, so it takes no location
            self.call_named("verb_eq", &[l.into(), r.into()]).unwrap().into_struct_value()
        } else {
            let helper = match op {
                BinOp::Add => "verb_add", BinOp::Sub => "verb_sub", BinOp::Mul => "verb_mul",
                BinOp::Div => "verb_div", BinOp::Mod => "verb_mod",
                BinOp::Lt => "verb_lt", BinOp::Gt => "verb_gt",
                BinOp::Le => "verb_le", BinOp::Ge => "verb_ge",
                BinOp::Concat => "verb_concat",
                BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or => unreachable!(),
            };
            let (lc, cc) = self.loc_consts(line, col);
            self.call_named(helper, &[l.into(), r.into(), lc.into(), cc.into()])
                .unwrap().into_struct_value()
        };
        self.call_named("verb_release_value", &[l.into()]);
        self.call_named("verb_release_value", &[r.into()]);
        if matches!(op, BinOp::Ne) {
            let p = self.payload_of(out);
            let flipped = self.builder.build_xor(
                p, self.ctx.i64_type().const_int(1, false), "ne").unwrap();
            return Ok(self.make_val(TAG_BOOL, flipped));
        }
        Ok(out)
    }

    fn gen_call(&mut self, callee: &Expr, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        // built-in print
        if let Expr::Var(name, ..) = callee {
            if name == "print" {
                if args.len() != 1 {
                    return Err(CompileError::new("print takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(self.nil_val());
            }
            if name == "len" {
                if args.len() != 1 {
                    return Err(CompileError::new("len takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_len", &[v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(rv);
            }
            if name == "get" {
                if args.len() != 2 {
                    return Err(CompileError::new("get takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_get", &[arr.into(), idx.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                self.call_named("verb_release_value", &[idx.into()]);
                return Ok(rv);
            }
            if name == "set" {
                if args.len() != 3 {
                    return Err(CompileError::new("set takes exactly 3 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let v = self.gen_expr(&args[2])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_set", &[arr.into(), idx.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                self.call_named("verb_release_value", &[idx.into()]);
                return Ok(rv);
            }
            if name == "push" {
                if args.len() != 2 {
                    return Err(CompileError::new("push takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let v = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_push", &[arr.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                return Ok(rv);
            }
            if name == "pop" {
                if args.len() != 1 {
                    return Err(CompileError::new("pop takes exactly 1 argument", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_pop", &[arr.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                return Ok(rv);
            }
            if name == "str_len" {
                if args.len() != 1 {
                    return Err(CompileError::new("str_len takes exactly 1 argument", line, col));
                }
                let s = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_str_len", &[s.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[s.into()]);
                return Ok(rv);
            }
            if name == "str_index" {
                if args.len() != 2 {
                    return Err(CompileError::new("str_index takes exactly 2 arguments", line, col));
                }
                let s = self.gen_expr(&args[0])?;
                let sub = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_str_index", &[s.into(), sub.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[s.into()]);
                self.call_named("verb_release_value", &[sub.into()]);
                return Ok(rv);
            }
            if name == "str_slice" {
                if args.len() != 3 {
                    return Err(CompileError::new("str_slice takes exactly 3 arguments", line, col));
                }
                let s = self.gen_expr(&args[0])?;
                let start = self.gen_expr(&args[1])?;
                let end = self.gen_expr(&args[2])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named(
                    "verb_str_slice", &[s.into(), start.into(), end.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[s.into()]);
                self.call_named("verb_release_value", &[start.into()]);
                self.call_named("verb_release_value", &[end.into()]);
                return Ok(rv);
            }
            if name == "str_split" {
                if args.len() != 2 {
                    return Err(CompileError::new("str_split takes exactly 2 arguments", line, col));
                }
                let s = self.gen_expr(&args[0])?;
                let sep = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_str_split", &[s.into(), sep.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[s.into()]);
                self.call_named("verb_release_value", &[sep.into()]);
                return Ok(rv);
            }
            // result-style error handling builtins (reuse the enum machinery;
            // `Ok`/`Err` are the built-in `Result` choice registered in `new`).
            if name == "is_err" {
                if args.len() != 1 {
                    return Err(CompileError::new("is_err takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                return Ok(self.gen_is_err(v));
            }
            if name == "err_kind" {
                if args.len() != 1 {
                    return Err(CompileError::new("err_kind takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                return Ok(self.gen_err_field(v, 0));
            }
            if name == "err_msg" {
                if args.len() != 1 {
                    return Err(CompileError::new("err_msg takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                return Ok(self.gen_err_field(v, 1));
            }
            // record construction: `Point(3, 4)` -> heap struct value
            if let Some(info) = self.records.get(name).cloned() {
                let n = info.fields.len();
                if args.len() != n {
                    return Err(CompileError::new(
                        format!("record '{name}' takes {n} field(s), got {}", args.len()),
                        line, col));
                }
                // Evaluate each field value first (owned temporaries); they are
                // moved into the struct's inline slots, so no extra retain --
                // an Expr::Var's load already retained, literals are owned.
                let mut vals = Vec::with_capacity(n);
                for a in args {
                    vals.push(self.gen_expr(a)?);
                }
                let sp = self.malloc_bytes(16 + (n as u64) * 16);
                let descp = self.builder.build_struct_gep(self.struct_hdr_ty, sp, 0, "descp").unwrap();
                self.builder.build_store(descp, info.descriptor).unwrap();
                let nfp = self.builder.build_struct_gep(self.struct_hdr_ty, sp, 1, "nfp").unwrap();
                self.builder.build_store(nfp, self.ctx.i64_type().const_int(n as u64, false)).unwrap();
                for (i, v) in vals.into_iter().enumerate() {
                    let slot = self.struct_field_slot(sp, self.ctx.i64_type().const_int(i as u64, false));
                    self.builder.build_store(slot, v).unwrap();
                }
                let bits = self.builder.build_ptr_to_int(sp, self.ctx.i64_type(), "sbits").unwrap();
                return Ok(self.make_val(TAG_STRUCT, bits));
            }
            // enum variant construction: `Circle(5)` -> heap enum value
            if let Some(info) = self.variants.get(name).cloned() {
                let n = info.fields.len();
                if args.len() != n {
                    return Err(CompileError::new(
                        format!("variant '{name}' takes {n} field(s), got {}", args.len()),
                        line, col));
                }
                // Evaluate each field value first (owned temporaries), moved
                // into the variant's inline slots -- same ownership handoff as
                // record construction.
                let mut vals = Vec::with_capacity(n);
                for a in args {
                    vals.push(self.gen_expr(a)?);
                }
                return Ok(self.build_variant(&info, vals));
            }
            let is_bound = self.lookup(name).is_some();
            if !is_bound && self.std_imports.iter().any(|m| m == "io") {
                if let Some(arity) = io_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, true, line, col);
                }
            }
            if !is_bound && self.std_imports.iter().any(|m| m == "map") {
                if let Some(arity) = map_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, false, line, col);
                }
            }
            if !is_bound && self.std_imports.iter().any(|m| m == "net") {
                if let Some(arity) = net_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, false, line, col);
                }
            }
            if !is_bound && !self.imports.is_empty() {
                return self.gen_extern_call(name, args, line, col);
            }
        }
        let cv = self.gen_expr(callee)?;
        let argc = self.ctx.i64_type().const_int(args.len() as u64, false);
        let (lc, cc) = self.loc_consts(line, col);
        let clos_ptr = self.call_named(
            "verb_check_call", &[cv.into(), argc.into(), lc.into(), cc.into()])
            .unwrap().into_pointer_value();

        let arr_ty = self.value_ty.array_type(args.len() as u32);
        let argv = self.entry_alloca(arr_ty.into(), "argv");
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a)?;
            let ap = unsafe {
                self.builder.build_in_bounds_gep(
                    self.value_ty, argv,
                    &[self.ctx.i64_type().const_int(i as u64, false)], "argp")
            }.unwrap();
            self.builder.build_store(ap, v).unwrap();
        }

        let fpp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 0, "fpp").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fpp, "fp").unwrap().into_pointer_value();
        let epp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 2, "epp").unwrap();
        let env = self.builder.build_load(self.ptr_ty, epp, "env").unwrap();
        self.call_named("verb_release_value", &[cv.into()]);

        let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
        let out = self.builder.build_indirect_call(
            fnty, fp, &[env.into(), argv.into()], "call").unwrap();
        Ok(out.try_as_basic_value().basic().unwrap().into_struct_value())
    }

    /// A call to one of a first-party `std` module's built-in functions
    /// (`io`'s, see runtime/verb_std_io.cpp, or `map`'s, see
    /// runtime/verb_map.cpp), reachable only when the corresponding
    /// `import std <module>;` is present. Arity is checked against the
    /// function's fixed, known signature (`IO_FUNCS`/`MAP_FUNCS`) on every
    /// call site — including the first — unlike `gen_extern_call`, whose
    /// arity is only checked against a prior call site of the same name,
    /// because generic `import mod` externs have no statically known
    /// signature to check against.
    fn gen_std_io_call(&mut self, name: &str, expected_arity: usize, args: &[Expr],
                       lift_errors: bool, line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        if args.len() != expected_arity {
            return Err(CompileError::new(
                format!(
                    "std io fn '{name}' takes {expected_arity} argument(s), got {}",
                    args.len()
                ),
                line, col,
            ));
        }
        let argvals: Vec<StructValue<'ctx>> =
            args.iter().map(|a| self.gen_expr(a)).collect::<Result<_, _>>()?;
        let fnv = match self.externs.get(name).copied() {
            Some(fnv) => fnv,
            None => {
                let param_tys: Vec<_> = (0..expected_arity).map(|_| self.value_ty.into()).collect();
                let fnty = self.value_ty.fn_type(&param_tys, false);
                let fnv = self.module.add_function(name, fnty, None);
                self.externs.insert(name.to_string(), fnv);
                fnv
            }
        };
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        let result = self.builder.build_call(fnv, &args_bv, "std_io_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
        for v in &argvals {
            self.call_named("verb_release_value", &[(*v).into()]);
        }
        // `io` functions signal failure by returning nil; lift that to a
        // built-in `Err`. `map` functions (lift_errors=false) keep nil as a
        // legitimate value (e.g. `map_get` of a missing key), unchanged.
        if lift_errors {
            Ok(self.lift_nil_to_err(result, name))
        } else {
            Ok(result)
        }
    }

    /// A call to a name that isn't a local variable or a known Verb `fn`,
    /// in a program that has at least one `import mod`. Declares (once
    /// per name, lazily, on first sight) a raw external function of type
    /// `VerbValue(VerbValue, VerbValue, ...)` — the same struct Verb's
    /// own runtime helpers already pass by value — and calls it directly.
    /// No unboxing: the extern C++ side receives Verb's tagged value
    /// as-is and is responsible for interpreting it (see runtime/verb.h).
    fn gen_extern_call(&mut self, name: &str, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        let argvals: Vec<StructValue<'ctx>> =
            args.iter().map(|a| self.gen_expr(a)).collect::<Result<_, _>>()?;
        let fnv = match self.externs.get(name).copied() {
            Some(fnv) => {
                if fnv.count_params() as usize != argvals.len() {
                    return Err(CompileError::new(
                        format!(
                            "extern fn '{name}' called with {} argument(s), previously called with {}",
                            argvals.len(), fnv.count_params()
                        ),
                        line, col,
                    ));
                }
                fnv
            }
            None => {
                // Footgun (accepted, v1): no symbol-existence checking here.
                // Any unresolved call-by-name in an `import`-using program
                // takes this path, including one that accidentally collides
                // with an already-declared symbol (e.g. `printf`, `malloc`).
                // LLVM silently auto-renames the duplicate declaration
                // (e.g. `printf.1`) instead of erroring, so the mistake
                // surfaces later as a confusing "undefined symbol" at link
                // time rather than a clear compile-time error. Per the
                // design spec this tradeoff is deliberate for v1, not an
                // oversight.
                let param_tys: Vec<_> = argvals.iter().map(|_| self.value_ty.into()).collect();
                let fnty = self.value_ty.fn_type(&param_tys, false);
                let fnv = self.module.add_function(name, fnty, None);
                self.externs.insert(name.to_string(), fnv);
                fnv
            }
        };
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        let result = self.builder.build_call(fnv, &args_bv, "extern_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
        for v in &argvals {
            self.call_named("verb_release_value", &[(*v).into()]);
        }
        Ok(result)
    }
}

/// Fixed name -> arity table for the `io` module's built-in functions
/// (see runtime/verb_std_io.cpp and the design spec). Unlike generic
/// `import mod` externs, these signatures are first-party and known
/// ahead of time, so arity is checked on every call site, not just
/// against a previous one.
const IO_FUNCS: &[(&str, usize)] = &[
    ("read_line", 0),
    ("file_read", 1),
    ("file_write", 2),
    ("file_append", 2),
    ("tcp_connect", 2),
    ("tcp_listen", 1),
    ("tcp_accept", 1),
    ("send_line", 2),
    ("recv_line", 1),
    ("close_conn", 1),
];

fn io_func_arity(name: &str) -> Option<usize> {
    IO_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// Fixed name -> arity table for the `map` module's built-in functions
/// (see runtime/verb_map.cpp and the design spec). See `IO_FUNCS`.
const MAP_FUNCS: &[(&str, usize)] = &[
    ("map_new", 0),
    ("map_set", 3),
    ("map_get", 2),
    ("map_has", 2),
    ("map_remove", 2),
    ("map_len", 1),
];

fn map_func_arity(name: &str) -> Option<usize> {
    MAP_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// Fixed name -> arity table for the `net` module's built-in functions
/// (see runtime/verb_std_net.cpp and the design spec). Blocking UDP
/// datagram helpers over POSIX sockets. Like `map`, failures return nil
/// (lift_errors=false), not a lifted `Err`. See `IO_FUNCS`.
const NET_FUNCS: &[(&str, usize)] = &[
    ("udp_socket", 0),
    ("udp_bind", 2),
    ("udp_send", 4),
    ("udp_recv", 1),
];

fn net_func_arity(name: &str) -> Option<usize> {
    NET_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkwell::context::Context;

    #[test]
    fn stamps_error_with_originating_file() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![
            Stmt::Assign { name: "x".to_string(), value: Expr::Int(1) },
            Stmt::ExprStmt(Expr::Var("undefined_name".to_string(), 3, 5)),
        ];
        let stmt_files = vec!["a.verb".to_string(), "b.verb".to_string()];

        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();

        assert_eq!(err.file, Some("b.verb".to_string()));
        assert_eq!(err.line, 3);
    }

    #[test]
    fn no_error_when_program_is_valid() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::Assign { name: "x".to_string(), value: Expr::Int(1) }];
        let stmt_files = vec!["a.verb".to_string()];

        assert!(cg.compile_program(&stmts, &stmt_files, &[], &[]).is_ok());
    }

    #[test]
    fn std_io_call_with_correct_arity_compiles_ok() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::Assign {
            name: "line".to_string(),
            value: Expr::Call {
                callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
                args: vec![],
                line: 1, col: 1,
            },
        }];
        let stmt_files = vec!["a.verb".to_string()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["io".to_string()]).is_ok());
    }

    #[test]
    fn std_io_arity_mismatch_is_a_compile_error() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
            args: vec![Expr::Int(1)],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg
            .compile_program(&stmts, &stmt_files, &[], &["io".to_string()])
            .unwrap_err();
        assert!(err.msg.contains("read_line"), "{}", err.msg);
        assert!(err.msg.contains("takes 0 argument"), "{}", err.msg);
    }

    #[test]
    fn std_io_name_ignored_without_import_std_io() {
        // 'read_line' with no `import std io;` present falls through to the
        // ordinary undefined-variable path, same as any unknown name.
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
        assert!(err.msg.contains("undefined variable"), "{}", err.msg);
    }

    #[test]
    fn std_map_call_with_correct_arity_compiles_ok() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("map_new".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["map".to_string()]).is_ok());
    }

    #[test]
    fn std_map_arity_mismatch_is_a_compile_error() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("map_get".to_string(), 1, 1)),
            args: vec![Expr::Int(1)],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg
            .compile_program(&stmts, &stmt_files, &[], &["map".to_string()])
            .unwrap_err();
        assert!(err.msg.contains("map_get"), "{}", err.msg);
        assert!(err.msg.contains("takes 2 argument"), "{}", err.msg);
    }

    #[test]
    fn std_map_name_ignored_without_import_std_map() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("map_new".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
        assert!(err.msg.contains("undefined variable"), "{}", err.msg);
    }
}
