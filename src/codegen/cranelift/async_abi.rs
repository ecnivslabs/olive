use super::CraneliftCodegen;
use super::imports::cl_type;
use crate::mir::MirFunction;
use crate::semantic::types::Type as OliveType;
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;
use cranelift_module::{FuncId, Linkage, Module};

pub(super) fn to_word(builder: &mut FunctionBuilder, value: Value, ty: &OliveType) -> Value {
    let native = cl_type(ty);
    match native {
        types::F32 => {
            let wide = builder.ins().fpromote(types::F64, value);
            builder.ins().bitcast(types::I64, MemFlags::new(), wide)
        }
        types::F64 => builder.ins().bitcast(types::I64, MemFlags::new(), value),
        types::I64 => value,
        _ if matches!(
            ty,
            OliveType::U8 | OliveType::U16 | OliveType::U32 | OliveType::Bool
        ) =>
        {
            builder.ins().uextend(types::I64, value)
        }
        _ => builder.ins().sextend(types::I64, value),
    }
}

pub(super) fn from_word(builder: &mut FunctionBuilder, value: Value, ty: &OliveType) -> Value {
    match cl_type(ty) {
        types::F32 => {
            let wide = builder.ins().bitcast(types::F64, MemFlags::new(), value);
            builder.ins().fdemote(types::F32, wide)
        }
        types::F64 => builder.ins().bitcast(types::F64, MemFlags::new(), value),
        types::I64 => value,
        native => builder.ins().ireduce(native, value),
    }
}

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn capture_async_arg(
        &mut self,
        builder: &mut FunctionBuilder,
        arg: Value,
        ty: &OliveType,
    ) -> Value {
        if !ty.needs_drop() {
            return arg;
        }
        if let OliveType::Struct(name, _, _) = super::imports::concrete_ty(ty)
            && self.c_struct_names.contains(name)
        {
            return arg;
        }
        let relocate = self
            .module
            .declare_func_in_func(self.func_ids["__olive_relocate_typed"], builder.func);
        if matches!(super::imports::concrete_ty(ty), OliveType::Fn(..)) {
            let copy = builder.create_block();
            let done = builder.create_block();
            builder.append_block_param(done, types::I64);
            let nonnull = builder.ins().icmp_imm(IntCC::NotEqual, arg, 0);
            builder
                .ins()
                .brif(nonnull, copy, &[], done, &[BlockArg::Value(arg)]);
            builder.seal_block(copy);
            builder.switch_to_block(copy);
            let tagged = builder.ins().load(types::I64, MemFlags::new(), arg, 16);
            let desc = builder.ins().band_imm(tagged, -2);
            let call = builder.ins().call(relocate, &[arg, desc]);
            let copied = builder.inst_results(call)[0];
            builder.ins().jump(done, &[BlockArg::Value(copied)]);
            builder.seal_block(done);
            builder.switch_to_block(done);
            return builder.block_params(done)[0];
        }
        let desc = super::imports::type_descriptor(
            ty,
            &self.struct_fields,
            &self.field_types,
            &self.enum_defs,
        );
        self.intern_attr_string(&desc);
        let desc_ptr =
            super::setup::strings::literal_body(builder, &mut self.module, self.string_ids[&desc]);
        let call = builder.ins().call(relocate, &[arg, desc_ptr]);
        builder.inst_results(call)[0]
    }

    pub(super) fn generate_async_invoke(
        &mut self,
        func: &MirFunction,
        body_id: FuncId,
        wrapper_id: FuncId,
    ) -> FuncId {
        let name = format!("__olive_async_invoke_{}", wrapper_id.as_u32());
        let mut ctx = self.module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));
        let invoke_id = self
            .module
            .declare_function(&name, Linkage::Local, &ctx.func.signature)
            .unwrap();
        let mut bctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.append_block_params_for_function_params(entry);
        let args_ptr = builder.block_params(entry)[0];
        let args: Vec<_> = (1..=func.arg_count)
            .map(|i| {
                let word =
                    builder
                        .ins()
                        .load(types::I64, MemFlags::new(), args_ptr, ((i - 1) * 8) as i32);
                from_word(&mut builder, word, &func.locals[i].ty)
            })
            .collect();
        let body = self.module.declare_func_in_func(body_id, builder.func);
        let call = builder.ins().call(body, &args);
        let value = builder.inst_results(call)[0];
        let word = to_word(&mut builder, value, &func.locals[0].ty);
        builder.ins().return_(&[word]);
        builder.finalize();
        self.module.define_function(invoke_id, &mut ctx).unwrap();
        invoke_id
    }
}
