//! Arity-specialized kwargs call lowering: the keyword-argument analogue of
//! `py_call.rs`'s `emit_py_call_arity`/`emit_py_call_method_arity`. A call
//! site with `positional + keyword <= 4` args skips `args_list`/
//! `kwvals_list` entirely, routing to one of `python_call_kw_arity.rs`'s
//! `__olive_py_call{,_method}_kw_v_p{P}_k{K}` runtime shells instead of the
//! list-based `olive_py_call_kw_v`/`olive_py_call_method_kw_v`. Split out
//! of `py_call.rs` to keep that file under the project's line-count cap.

use super::super::MirBuilder;
use super::py_call::{PyCallArgs, PyCallFlavor};
use crate::mir::ir::*;
use crate::semantic::types::Type;
use crate::span::Span;

impl<'a> MirBuilder<'a> {
    /// Emits a call through the arity-specialized
    /// `__olive_py_call_kw_v_p{P}_k{K}(_safe)` shells (`positional +
    /// keyword <= 4`): every positional and keyword value passed straight
    /// as a call register, no `args_list`/`kwvals_list` aggregate. The
    /// keyword-argument analogue of `emit_py_call_arity`'s no-allocation
    /// shape. Never fuses the result -- same as the list-based
    /// `olive_py_call_kw_v`, the assigned local is always a plain handle.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_py_call_kw_arity(
        &mut self,
        callee_op: Operand,
        pos_ops: Vec<Operand>,
        kw_vals: Vec<Operand>,
        kw_names_packed: String,
        coll_tags: i64,
        arg_tags: i64,
        kw_coll_tags: i64,
        kw_arg_tags: i64,
        flavor: PyCallFlavor,
        span: Span,
    ) -> Operand {
        let name = Self::kw_arity_call_name(pos_ops.len(), kw_vals.len(), &flavor);

        let mut call_operands = vec![
            callee_op,
            Operand::Constant(Constant::Int(coll_tags)),
            Operand::Constant(Constant::Int(arg_tags)),
            Operand::Constant(Constant::Str(kw_names_packed)),
            Operand::Constant(Constant::Int(kw_coll_tags)),
            Operand::Constant(Constant::Int(kw_arg_tags)),
        ];
        call_operands.extend(pos_ops);
        call_operands.extend(kw_vals);
        if matches!(flavor, PyCallFlavor::Unsafe) {
            call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
        }

        let result = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Runtime entry-point name for one `(positional, keyword)` shape,
    /// shared naming convention with `kw_arity_method_call_name`.
    fn kw_arity_call_name(p: usize, k: usize, flavor: &PyCallFlavor) -> &'static str {
        match (p, k, flavor) {
            (0, 1, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p0_k1",
            (0, 1, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p0_k1_safe",
            (0, 2, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p0_k2",
            (0, 2, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p0_k2_safe",
            (0, 3, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p0_k3",
            (0, 3, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p0_k3_safe",
            (0, 4, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p0_k4",
            (0, 4, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p0_k4_safe",
            (1, 1, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p1_k1",
            (1, 1, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p1_k1_safe",
            (1, 2, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p1_k2",
            (1, 2, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p1_k2_safe",
            (1, 3, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p1_k3",
            (1, 3, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p1_k3_safe",
            (2, 1, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p2_k1",
            (2, 1, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p2_k1_safe",
            (2, 2, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p2_k2",
            (2, 2, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p2_k2_safe",
            (3, 1, PyCallFlavor::Unsafe) => "__olive_py_call_kw_v_p3_k1",
            (3, 1, PyCallFlavor::Safe) => "__olive_py_call_kw_v_p3_k1_safe",
            (p, k, _) => unreachable!("kw_arity_call_name: shape ({p},{k}) out of range"),
        }
    }

    /// Keyword-argument analogue of `emit_py_call_method_arity`: every
    /// positional and keyword value passed straight as a call register, no
    /// `args_list`/`kwvals_list` aggregate. Mirrors `emit_py_call_kw_arity`
    /// with the receiver and attribute name as two extra leading operands.
    /// Never fuses the result -- same as `emit_py_call_method_kw_v`.
    pub(super) fn emit_py_call_method_kw_arity(
        &mut self,
        obj_op: Operand,
        attr: String,
        call_args: PyCallArgs,
        flavor: PyCallFlavor,
        span: Span,
    ) -> Operand {
        let (pos_ops, kw_vals, kw_names_packed, arg_tags, coll_tags, kw_arg_tags, kw_coll_tags) =
            call_args.into_kw_parts();

        let name = Self::kw_arity_method_call_name(pos_ops.len(), kw_vals.len(), &flavor);
        let kwnames = kw_names_packed.unwrap_or_default();

        let mut call_operands = vec![
            obj_op,
            Operand::Constant(Constant::Str(attr)),
            Operand::Constant(Constant::Int(coll_tags)),
            Operand::Constant(Constant::Int(arg_tags)),
            Operand::Constant(Constant::Str(kwnames)),
            Operand::Constant(Constant::Int(kw_coll_tags)),
            Operand::Constant(Constant::Int(kw_arg_tags)),
        ];
        call_operands.extend(pos_ops);
        call_operands.extend(kw_vals);
        if matches!(flavor, PyCallFlavor::Unsafe) {
            call_operands.push(Operand::Constant(Constant::Str(self.call_loc_str(span))));
        }

        let result = self.new_local(Type::PyObject, None, true);
        self.push_statement(
            StatementKind::Assign(
                result,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function(name.to_string())),
                    args: call_operands,
                },
            ),
            span,
        );
        self.operand_for_local(result)
    }

    /// Runtime entry-point name for one `(positional, keyword)` shape, the
    /// `__olive_py_call_method_kw_v_p{P}_k{K}` counterpart of
    /// `kw_arity_call_name`.
    fn kw_arity_method_call_name(p: usize, k: usize, flavor: &PyCallFlavor) -> &'static str {
        match (p, k, flavor) {
            (0, 1, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p0_k1",
            (0, 1, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p0_k1_safe",
            (0, 2, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p0_k2",
            (0, 2, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p0_k2_safe",
            (0, 3, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p0_k3",
            (0, 3, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p0_k3_safe",
            (0, 4, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p0_k4",
            (0, 4, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p0_k4_safe",
            (1, 1, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p1_k1",
            (1, 1, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p1_k1_safe",
            (1, 2, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p1_k2",
            (1, 2, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p1_k2_safe",
            (1, 3, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p1_k3",
            (1, 3, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p1_k3_safe",
            (2, 1, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p2_k1",
            (2, 1, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p2_k1_safe",
            (2, 2, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p2_k2",
            (2, 2, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p2_k2_safe",
            (3, 1, PyCallFlavor::Unsafe) => "__olive_py_call_method_kw_v_p3_k1",
            (3, 1, PyCallFlavor::Safe) => "__olive_py_call_method_kw_v_p3_k1_safe",
            (p, k, _) => unreachable!("kw_arity_method_call_name: shape ({p},{k}) out of range"),
        }
    }
}
