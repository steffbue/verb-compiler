use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{PointerType, StructType};
use inkwell::values::{IntValue, PointerValue, StructValue};
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
            scopes: Vec::new(), fn_counter: 0,
        };
        cg.declare_libc();
        cg.build_print_fn();
        cg.build_truthy_fn();
        cg.build_arith_fns();
        cg.build_cmp_fns();
        cg.build_eq_fn();
        cg.build_concat_fn();
        cg.build_neg_fn();
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

    fn abort(&self, msg: &str) {
        let s = self.cstr(&format!("runtime error: {msg}\n"));
        self.call_named("printf", &[s.into()]);
        self.call_named("exit", &[self.ctx.i32_type().const_int(1, false).into()]);
        self.builder.build_unreachable().unwrap();
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
        for (name, op) in [("verb_add", BinOp::Add), ("verb_sub", BinOp::Sub),
                           ("verb_mul", BinOp::Mul), ("verb_div", BinOp::Div),
                           ("verb_mod", BinOp::Mod)] {
            self.build_arith_fn(name, op);
        }
    }

    fn build_arith_fn(&self, name: &str, op: BinOp) {
        use inkwell::IntPredicate::*;
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function(name, fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chknum");
        let flt_bb = self.ctx.append_basic_block(f, "float");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
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
            self.abort("division by zero");
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
            self.abort("division by zero");
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
        self.abort("operands must be numbers");
    }

    fn build_cmp_fns(&self) {
        use inkwell::{FloatPredicate as FP, IntPredicate as IP};
        for (name, ip, fp) in [
            ("verb_lt", IP::SLT, FP::OLT), ("verb_gt", IP::SGT, FP::OGT),
            ("verb_le", IP::SLE, FP::OLE), ("verb_ge", IP::SGE, FP::OGE),
        ] {
            let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
            let f = self.module.add_function(name, fnty, None);
            let entry = self.ctx.append_basic_block(f, "entry");
            let int_bb = self.ctx.append_basic_block(f, "int");
            let chk_bb = self.ctx.append_basic_block(f, "chk");
            let flt_bb = self.ctx.append_basic_block(f, "flt");
            let err_bb = self.ctx.append_basic_block(f, "err");

            self.builder.position_at_end(entry);
            let a = f.get_nth_param(0).unwrap().into_struct_value();
            let b = f.get_nth_param(1).unwrap().into_struct_value();
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
            self.abort("operands must be numbers");
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
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function("verb_concat", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
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
        self.abort("operands of 'c' must be strings");
    }

    fn build_neg_fn(&self) {
        use inkwell::IntPredicate::*;
        let fnty = self.value_ty.fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_neg", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chk");
        let flt_bb = self.ctx.append_basic_block(f, "flt");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
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
        self.abort("operand must be a number");
    }

    // ----- program -----

    pub fn compile_program(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.scopes.push(HashMap::new());
        self.gen_stmts(stmts)?;
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

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::ExprStmt(e) => { self.gen_expr(e)?; Ok(()) }
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
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
            Expr::Binary { op, lhs, rhs } => self.gen_binary(*op, lhs, rhs),
            Expr::Unary { op, expr } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => Ok(self.call_named("verb_neg", &[v.into()])
                        .unwrap().into_struct_value()),
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
            Expr::Call { callee, args, line, col } => self.gen_call(callee, args, *line, *col),
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
        }
    }

    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr)
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
        let helper = match op {
            BinOp::Add => "verb_add", BinOp::Sub => "verb_sub", BinOp::Mul => "verb_mul",
            BinOp::Div => "verb_div", BinOp::Mod => "verb_mod",
            BinOp::Lt => "verb_lt", BinOp::Gt => "verb_gt",
            BinOp::Le => "verb_le", BinOp::Ge => "verb_ge",
            BinOp::Eq | BinOp::Ne => "verb_eq",
            BinOp::Concat => "verb_concat",
            BinOp::And | BinOp::Or => unreachable!(),
        };
        let out = self.call_named(helper, &[l.into(), r.into()]).unwrap().into_struct_value();
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
        }
        Err(CompileError::new("codegen for user calls arrives in Task 8", line, col))
    }
}
