use super::super::CraneliftCodegen;
use super::super::imports::is_float_op;
use crate::mir::MirFunction;
use crate::mir::StatementKind;
use cranelift_module::{DataDescription, Linkage, Module};

impl<M: Module> CraneliftCodegen<M> {
    pub(crate) fn intern_attr_string(&mut self, attr: &str) {
        if self.string_ids.contains_key(attr) {
            return;
        }
        let mut data_ctx = DataDescription::new();
        let mut bytes = attr.as_bytes().to_vec();
        bytes.push(0);
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        data_ctx.define(bytes.into_boxed_slice());
        let name = format!("str_{}", self.string_ids.len());
        let id = self
            .module
            .declare_data(&name, Linkage::Export, false, false)
            .unwrap();
        self.module.define_data(id, &data_ctx).unwrap();
        self.string_ids.insert(attr.to_string(), id);
    }

    /// Interns a read-only `file:line:col` string and returns a tagged Olive
    /// string pointer's backing data id. Reused for every fault site sharing
    /// the same source location.
    fn intern_loc(&mut self, span: crate::span::Span) -> cranelift_module::DataId {
        let loc = match self.file_names.get(&span.file_id) {
            Some(file) => format!("{}:{}:{}", file, span.line, span.col),
            None => format!("{}:{}", span.line, span.col),
        };
        if let Some(&id) = self.loc_ids.get(&span) {
            return id;
        }
        let mut data_ctx = DataDescription::new();
        let mut bytes = loc.into_bytes();
        bytes.push(0);
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        data_ctx.define(bytes.into_boxed_slice());
        let name = format!("loc_{}", self.loc_ids.len());
        let id = self
            .module
            .declare_data(&name, Linkage::Export, false, false)
            .unwrap();
        self.module.define_data(id, &data_ctx).unwrap();
        self.loc_ids.insert(span, id);
        id
    }

    /// Records source locations for every fault-prone statement (index reads
    /// and writes) so a runtime bounds or nil-index panic can point at the line.
    pub(super) fn collect_locs(&mut self, func: &MirFunction) {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, crate::mir::Rvalue::GetIndex(..))
                    | StatementKind::SetIndex(..) => {
                        self.intern_loc(stmt.span);
                    }
                    StatementKind::Assign(
                        _,
                        crate::mir::Rvalue::BinaryOp(
                            crate::parser::BinOp::Div | crate::parser::BinOp::Mod,
                            lhs,
                            _,
                        ),
                    ) if !is_float_op(func, lhs) => {
                        self.intern_loc(stmt.span);
                    }
                    StatementKind::GenCheck { value, .. } => {
                        self.intern_loc(stmt.span);
                        if let Some(name) = func.locals[value.0].name.clone() {
                            self.intern_attr_string(&name);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn collect_strings(&mut self, func: &MirFunction) {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => {
                        self.collect_strings_in_rvalue(rval);
                        self.collect_type_descriptor(func, rval);
                    }
                    StatementKind::SetAttr(_, attr, val_op) => {
                        self.intern_attr_string(attr);
                        self.collect_strings_in_operand(val_op);
                    }
                    StatementKind::SetIndex(obj_op, idx_op, val_op, _) => {
                        self.collect_strings_in_operand(obj_op);
                        self.collect_strings_in_operand(idx_op);
                        self.collect_strings_in_operand(val_op);
                        self.collect_dict_key_descriptor(func, obj_op);
                    }
                    StatementKind::Drop(local) => {
                        use super::super::imports::{drop_descriptor_type, type_descriptor};
                        let ty = &func.locals[local.0].ty;
                        if ty.is_move_type()
                            && let Some(desc_ty) = drop_descriptor_type(ty, &self.struct_fields)
                        {
                            let desc = type_descriptor(
                                desc_ty,
                                &self.struct_fields,
                                &self.field_types,
                                &self.enum_defs,
                            );
                            self.intern_attr_string(&desc);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Interns a `Dict`-typed operand's key descriptor when the key type
    /// needs structural hash+eq. Shared by `SetIndex` (`collect_strings`)
    /// and `GetIndex` (`collect_type_descriptor` below).
    fn collect_dict_key_descriptor(&mut self, func: &MirFunction, obj_op: &crate::mir::Operand) {
        use super::super::imports::{needs_structural_key, operand_static_type, type_descriptor};
        let mut ty = operand_static_type(obj_op, func);
        while let crate::semantic::types::Type::Ref(inner)
        | crate::semantic::types::Type::MutRef(inner) = ty
        {
            ty = *inner;
        }
        if let crate::semantic::types::Type::Dict(k, _) = &ty
            && needs_structural_key(k)
        {
            let desc = type_descriptor(k, &self.struct_fields, &self.field_types, &self.enum_defs);
            self.intern_attr_string(&desc);
        }
    }

    fn collect_type_descriptor(&mut self, func: &MirFunction, rval: &crate::mir::Rvalue) {
        use super::super::imports::{
            needs_structural_key, needs_type_descriptor, operand_static_type, type_descriptor,
        };
        use crate::mir::{Constant, Operand, Rvalue};
        if let Rvalue::GetIndex(obj_op, _, _) = rval {
            self.collect_dict_key_descriptor(func, obj_op);
            return;
        }
        if let Rvalue::BinaryOp(op, _lhs, rhs) = rval
            && matches!(op, crate::parser::BinOp::In | crate::parser::BinOp::NotIn)
        {
            let mut ty = operand_static_type(rhs, func);
            while let crate::semantic::types::Type::Ref(inner)
            | crate::semantic::types::Type::MutRef(inner) = ty
            {
                ty = *inner;
            }
            let key_ty = match &ty {
                crate::semantic::types::Type::Dict(k, _) => Some(k.as_ref()),
                crate::semantic::types::Type::Set(e) => Some(e.as_ref()),
                _ => None,
            };
            if let Some(k) = key_ty
                && needs_structural_key(k)
            {
                let desc =
                    type_descriptor(k, &self.struct_fields, &self.field_types, &self.enum_defs);
                self.intern_attr_string(&desc);
            }
            return;
        }
        if let Rvalue::Aggregate(kind, ops) = rval {
            use crate::mir::ir::AggregateKind;
            // Dict key is `ops[0]` (key, value, key, value, ...); set
            // element is `ops[0]` directly. Every literal shares one static
            // type, so the first operand alone decides it.
            let key_pos = match kind {
                AggregateKind::Dict => ops.first(),
                AggregateKind::Set => ops.first(),
                _ => None,
            };
            if let Some(op) = key_pos {
                let ty = operand_static_type(op, func);
                if needs_structural_key(&ty) {
                    let desc = type_descriptor(
                        &ty,
                        &self.struct_fields,
                        &self.field_types,
                        &self.enum_defs,
                    );
                    self.intern_attr_string(&desc);
                }
            }
            return;
        }
        let Rvalue::Call { func: callee, args } = rval else {
            return;
        };
        let Operand::Constant(Constant::Function(name)) = callee else {
            return;
        };
        let typed_list_arg = match name.as_str() {
            "__olive_list_concat_typed" if args.len() == 2 => Some(0usize),
            "__olive_list_getslice_typed" if args.len() == 5 => Some(0usize),
            "__olive_list_repeat_typed" if args.len() == 2 => Some(0usize),
            "__olive_list_extend_typed" if args.len() == 2 => Some(1usize),
            "__olive_obj_update_typed" if args.len() == 2 => Some(1usize),
            "__olive_set_add_typed"
            | "__olive_set_remove_typed"
            | "__olive_set_contains_typed"
            | "__olive_obj_get_typed"
            | "__olive_list_count_typed"
                if args.len() == 2 =>
            {
                Some(1usize)
            }
            "__olive_obj_get_default_typed"
            | "__olive_list_index_typed"
            | "__olive_set_remove_checked_typed"
            | "__olive_obj_pop_checked_typed"
            | "__olive_obj_pop_default_typed"
            | "__olive_obj_setdefault_typed"
                if args.len() == 3 =>
            {
                Some(1usize)
            }
            _ => None,
        };
        if let Some(pos) = typed_list_arg {
            let mut ty = operand_static_type(&args[pos], func);
            while let crate::semantic::types::Type::Ref(inner)
            | crate::semantic::types::Type::MutRef(inner) = ty
            {
                ty = *inner;
            }
            let desc =
                type_descriptor(&ty, &self.struct_fields, &self.field_types, &self.enum_defs);
            self.intern_attr_string(&desc);
            return;
        }
        if name == "__olive_eq_typed" && args.len() == 2 {
            let ty = operand_static_type(&args[0], func);
            let desc =
                type_descriptor(&ty, &self.struct_fields, &self.field_types, &self.enum_defs);
            self.intern_attr_string(&desc);
            return;
        }
        let is_copy_intrinsic = name == "__olive_copy_typed" || name == "__olive_relocate_typed";
        if name != "print" && name != "str" && !is_copy_intrinsic {
            return;
        }
        if args.len() != 1 {
            return;
        }
        // Operand shape must match `translate_call`'s own `arg_type` derivation
        // exactly (`operand_static_type`) -- collection ran ahead of a
        // constant-propagated `Copy(local)` here once and silently skipped
        // interning a folded `Constant` argument, crashing codegen later.
        let ty = operand_static_type(&args[0], func);
        if is_copy_intrinsic || needs_type_descriptor(&ty) {
            let desc =
                type_descriptor(&ty, &self.struct_fields, &self.field_types, &self.enum_defs);
            self.intern_attr_string(&desc);
        }
    }

    fn collect_strings_in_rvalue(&mut self, rval: &crate::mir::Rvalue) {
        use crate::mir::Rvalue;
        match rval {
            Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
                self.collect_strings_in_operand(op);
            }
            Rvalue::GetAttr(op, attr) => {
                self.collect_strings_in_operand(op);
                self.intern_attr_string(attr);
            }
            Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r, _) => {
                self.collect_strings_in_operand(l);
                self.collect_strings_in_operand(r);
            }
            Rvalue::Call { func, args } => {
                self.collect_strings_in_operand(func);
                for arg in args {
                    self.collect_strings_in_operand(arg);
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.collect_strings_in_operand(op);
                }
            }
            _ => {}
        }
    }

    fn collect_strings_in_operand(&mut self, op: &crate::mir::Operand) {
        use crate::mir::{Constant, Operand};
        if let Operand::Constant(Constant::Str(s)) = op
            && !self.string_ids.contains_key(s)
        {
            let mut data_ctx = DataDescription::new();
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            if bytes.len() % 2 != 0 {
                bytes.push(0);
            }
            data_ctx.define(bytes.into_boxed_slice());

            let name = format!("str_{}", self.string_ids.len());
            let id = self
                .module
                .declare_data(&name, Linkage::Export, false, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
            self.string_ids.insert(s.clone(), id);
        }
    }
}
