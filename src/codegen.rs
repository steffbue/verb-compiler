use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
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
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    functions: HashMap<String, (FunctionValue<'ctx>, usize)>,
    externs: HashMap<String, FunctionValue<'ctx>>,
    imports: Vec<String>,
    fn_depth: u32,
    fn_counter: u32,
}

impl<'ctx> Codegen<'ctx> {
    pub fn new(ctx: &'ctx Context) -> Self {
        let module = ctx.create_module("verb");
        let builder = ctx.create_builder();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let value_ty = ctx.struct_type(&[ctx.i8_type().into(), ctx.i64_type().into()], false);
        let closure_ty =
            ctx.struct_type(&[ptr_ty.into(), ctx.i64_type().into(), ptr_ty.into()], false);
        let cg = Self {
            ctx, module, builder, value_ty, closure_ty, ptr_ty,
            scopes: Vec::new(), functions: HashMap::new(), externs: HashMap::new(),
            imports: Vec::new(), fn_depth: 0, fn_counter: 0,
        };
        cg.declare_libc();
        cg.build_type_name_fn();
        cg.build_print_fn();
        cg.build_truthy_fn();
        cg.build_arith_fns();
        cg.build_cmp_fns();
        cg.build_eq_fn();
        cg.build_concat_fn();
        cg.build_neg_fn();
        cg.build_check_call_fn();
        cg
    }

    pub fn module(&self) -> &Module<'ctx> { &self.module }

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
                          (TAG_FLOAT, "float"), (TAG_STR, "string"), (TAG_CLOSURE, "fn")] {
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

    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("malloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }

    // ----- generated runtime helper: verb_print(value) -----

    fn build_print_fn(&self) {
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_print", fnty, None);
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
        let done = self.ctx.append_basic_block(f, "done");

        let i8t = self.ctx.i8_type();
        self.builder.build_switch(tag, done, &[
            (i8t.const_int(TAG_NIL, false), nil_bb),
            (i8t.const_int(TAG_BOOL, false), bool_bb),
            (i8t.const_int(TAG_INT, false), int_bb),
            (i8t.const_int(TAG_FLOAT, false), float_bb),
            (i8t.const_int(TAG_STR, false), str_bb),
            (i8t.const_int(TAG_CLOSURE, false), clos_bb),
        ]).unwrap();

        self.builder.position_at_end(nil_bb);
        self.call_named("printf", &[self.cstr("nil\n").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(bool_bb);
        let is_true = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, pay, self.ctx.i64_type().const_zero(), "istrue").unwrap();
        let ts = self.cstr("true\n");
        let fs = self.cstr("false\n");
        let sel = self.builder.build_select(is_true, ts, fs, "boolstr").unwrap();
        self.call_named("printf", &[sel.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(int_bb);
        self.call_named("printf", &[self.cstr("%lld\n").into(), pay.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(float_bb);
        let fv = self.builder.build_bit_cast(pay, self.ctx.f64_type(), "f").unwrap();
        self.call_named("printf", &[self.cstr("%g\n").into(), fv.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(pay, self.ptr_ty, "sptr").unwrap();
        self.call_named("printf", &[self.cstr("%s\n").into(), sp.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(clos_bb);
        self.call_named("printf", &[self.cstr("<fn>\n").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(done);
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
        let ir = match op {
            BinOp::Add => self.builder.build_int_add(pa, pb, "r").unwrap(),
            BinOp::Sub => self.builder.build_int_sub(pa, pb, "r").unwrap(),
            BinOp::Mul => self.builder.build_int_mul(pa, pb, "r").unwrap(),
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
        let sum = self.builder.build_int_add(la, lb, "sum").unwrap();
        let size = self.builder.build_int_add(sum, self.ctx.i64_type().const_int(1, false), "sz").unwrap();
        let buf = self.call_named("malloc", &[size.into()]).unwrap().into_pointer_value();
        self.call_named("strcpy", &[buf.into(), sa.into()]);
        self.call_named("strcat", &[buf.into(), sb.into()]);
        let bits = self.builder.build_ptr_to_int(buf, self.ctx.i64_type(), "bits").unwrap();
        let rv = self.make_val(TAG_STR, bits);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort_at(line, col, "'join' needs strings, got %s and %s",
                      &[self.type_name(ta), self.type_name(tb)]);
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

    /// Heap-allocate a closure struct { fn_ptr, arity, env } and wrap it as a tagged value.
    fn make_closure(&self, fnv: FunctionValue<'ctx>, arity: usize) -> StructValue<'ctx> {
        let p = self.malloc_bytes(24);
        let fp = fnv.as_global_value().as_pointer_value();
        self.builder.build_store(p, fp).unwrap();
        let ap = self.builder.build_struct_gep(self.closure_ty, p, 1, "ap").unwrap();
        self.builder.build_store(ap, self.ctx.i64_type().const_int(arity as u64, false)).unwrap();
        let ep = self.builder.build_struct_gep(self.closure_ty, p, 2, "ep").unwrap();
        self.builder.build_store(ep, self.ptr_ty.const_null()).unwrap();
        let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "cbits").unwrap();
        self.make_val(TAG_CLOSURE, bits)
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

    pub fn compile_program(&mut self, program: &Program) -> Result<(), CompileError> {
        self.imports = program.imports.clone();
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.scopes.push(HashMap::new());
        self.gen_stmts(&program.body)?;
        self.scopes.pop();
        if self.cur_block_open() {
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

    fn lookup(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Hint for an unresolved name: keyword rename, else closest known name.
    fn name_hint(&self, name: &str) -> Option<String> {
        if let Some(new) = crate::lexer::renamed_keyword(name) {
            return Some(format!("'{name}' was renamed to '{new}'"));
        }
        let best = self.scopes.iter().flat_map(|s| s.keys())
            .chain(self.functions.keys())
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
            Stmt::ExprStmt(e) => { self.gen_expr(e)?; Ok(()) }
            Stmt::Assign { name, value } => {
                let v = self.gen_expr(value)?;
                let cell = self.malloc_bytes(16);
                self.builder.build_store(cell, v).unwrap();
                self.scopes.last_mut().unwrap().insert(name.clone(), cell);
                Ok(())
            }
            Stmt::Declare { name } => {
                let cell = self.malloc_bytes(16);
                self.builder.build_store(cell, self.nil_val()).unwrap();
                self.scopes.last_mut().unwrap().insert(name.clone(), cell);
                Ok(())
            }
            Stmt::Reassign { name, value, line, col } => {
                let cell = self.lookup(name).ok_or_else(|| {
                    self.undefined_var(name, *line, *col)
                        .with_hint("declare new variables with 'assign' or 'declare'".to_string())
                })?;
                let v = self.gen_expr(value)?;
                self.builder.build_store(cell, v).unwrap();
                Ok(())
            }
            Stmt::Block(stmts) => {
                self.scopes.push(HashMap::new());
                let r = self.gen_stmts(stmts);
                self.scopes.pop();
                r
            }
            Stmt::If { cond, then_body, else_body } => {
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let then_bb = self.ctx.append_basic_block(f, "if.then");
                let else_bb = self.ctx.append_basic_block(f, "if.else");
                let merge = self.ctx.append_basic_block(f, "if.end");
                self.builder.build_conditional_branch(t, then_bb, else_bb).unwrap();

                self.builder.position_at_end(then_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(then_body)?;
                self.scopes.pop();
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }

                self.builder.position_at_end(else_bb);
                if let Some(eb) = else_body {
                    self.scopes.push(HashMap::new());
                    self.gen_stmts(eb)?;
                    self.scopes.pop();
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
                self.builder.build_conditional_branch(t, body_bb, end_bb).unwrap();

                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                self.scopes.pop();
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
                // register before compiling the body so recursion resolves
                self.functions.insert(name.clone(), (fnv, params.len()));
                // bind the name as a first-class closure value in the current scope
                let clos = self.make_closure(fnv, params.len());
                let cell = self.malloc_bytes(16);
                self.builder.build_store(cell, clos).unwrap();
                self.scopes.last_mut().unwrap().insert(name.clone(), cell);

                let saved_bb = self.builder.get_insert_block().unwrap();
                let saved_scopes = std::mem::take(&mut self.scopes);
                self.fn_depth += 1;

                let entry = self.ctx.append_basic_block(fnv, "entry");
                self.builder.position_at_end(entry);
                let argv = fnv.get_nth_param(1).unwrap().into_pointer_value();
                let mut scope = HashMap::new();
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
                self.builder.build_return(Some(&v)).unwrap();
                Ok(())
            }
        }
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
                let p = self.cstr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
            Expr::Bool(b) => Ok(self.make_val(TAG_BOOL, self.ctx.i64_type().const_int(*b as u64, false))),
            Expr::Nil => Ok(self.nil_val()),
            Expr::Var(name, line, col) => {
                if let Some(cell) = self.lookup(name) {
                    return Ok(self.builder.build_load(self.value_ty, cell, name)
                        .unwrap().into_struct_value());
                }
                // function names resolve even where their closure cell is out of scope
                // (e.g. recursion — the defining function's locals are not captured)
                if let Some((fnv, arity)) = self.functions.get(name).copied() {
                    return Ok(self.make_closure(fnv, arity));
                }
                Err(self.undefined_var(name, *line, *col))
            }
            Expr::Binary { op, lhs, rhs, line, col } => self.gen_binary(*op, lhs, rhs, *line, *col),
            Expr::Unary { op, expr, line, col } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let (lc, cc) = self.loc_consts(*line, *col);
                        Ok(self.call_named("verb_neg", &[v.into(), lc.into(), cc.into()])
                            .unwrap().into_struct_value())
                    }
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
            Expr::Call { callee, args, line, col } => self.gen_call(callee, args, *line, *col),
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
                return Ok(self.nil_val());
            }
            if !self.imports.is_empty()
                && self.lookup(name).is_none()
                && !self.functions.contains_key(name)
            {
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

        let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
        let out = self.builder.build_indirect_call(
            fnty, fp, &[env.into(), argv.into()], "call").unwrap();
        Ok(out.try_as_basic_value().basic().unwrap().into_struct_value())
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
        Ok(self.builder.build_call(fnv, &args_bv, "extern_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value())
    }
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
