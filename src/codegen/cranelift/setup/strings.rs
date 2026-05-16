use super::super::CraneliftCodegen;
use super::super::imports::is_float_op;
use crate::mir::MirFunction;
use crate::mir::StatementKind;
use cranelift_module::{DataDescription, Linkage, Module};

impl<M: Module> CraneliftCodegen<M> {
    pub(super) fn intern_attr_string(&mut self, attr: &str) {
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

    fn collect_type_descriptor(&mut self, func: &MirFunction, rval: &crate::mir::Rvalue) {
        use super::super::imports::{needs_type_descriptor, type_descriptor};
        use crate::mir::{Constant, Operand, Rvalue};
        let Rvalue::Call { func: callee, args } = rval else {
            return;
        };
        let Operand::Constant(Constant::Function(name)) = callee else {
            return;
        };
        if name != "print" && name != "str" && name != "__olive_copy_typed" {
            return;
        }
        if args.len() != 1 {
            return;
        }
        let ty = match &args[0] {
            Operand::Copy(l) | Operand::Move(l) => &func.locals[l.0].ty,
            _ => return,
        };
        // A copy-on-escape always needs its descriptor, even for a bare string
        // whose print/format path would not.
        if name == "__olive_copy_typed" || needs_type_descriptor(ty) {
            let desc = type_descriptor(ty, &self.struct_fields, &self.field_types, &self.enum_defs);
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
