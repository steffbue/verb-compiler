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
            Expr::Call { callee, args, line, col } => self.gen_call(callee, args, *line, *col),
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
        }
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
