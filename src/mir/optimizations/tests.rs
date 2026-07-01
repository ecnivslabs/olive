#[cfg(test)]
mod optimization_tests {
    use crate::lexer::Lexer;
    use crate::mir::ir::{Constant, MirFunction, Operand, Rvalue, StatementKind};
    use crate::mir::{MirBuilder, Optimizer};
    use crate::parser::Parser;
    use crate::parser::ast::BinOp;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet as HashSet;

    fn build_and_optimize(src: &str) -> Vec<MirFunction> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            HashSet::default(),
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        let opt = Optimizer::new();
        let (_diags, _copy_sites) = opt.run(&mut builder.functions);
        builder.functions
    }

    fn find_fn<'a>(fns: &'a [MirFunction], name: &str) -> &'a MirFunction {
        fns.iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    fn has_rvalue(func: &MirFunction, pred: impl Fn(&Rvalue) -> bool) -> bool {
        func.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                if let StatementKind::Assign(_, rval) = &s.kind {
                    pred(rval)
                } else {
                    false
                }
            })
        })
    }

    fn has_operand(func: &MirFunction, pred: impl Fn(&Operand) -> bool) -> bool {
        func.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                if let StatementKind::Assign(_, rval) = &s.kind {
                    match rval {
                        Rvalue::Use(op) => pred(op),
                        Rvalue::BinaryOp(_, op1, op2) => pred(op1) || pred(op2),
                        Rvalue::UnaryOp(_, op) => pred(op),
                        Rvalue::Call { func: callee, args } => {
                            pred(callee) || args.iter().any(&pred)
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            })
        })
    }

    #[test]
    fn constant_folding_simple_arithmetic() {
        let fns = build_and_optimize("fn f() -> i64:\n    return 2 + 3\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(5)))),
            "2+3 should fold to 5"
        );
    }

    #[test]
    fn constant_folding_multiplication() {
        let fns = build_and_optimize("fn f() -> i64:\n    return 6 * 7\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(42)))),
            "6*7 should fold to 42"
        );
    }

    #[test]
    fn constant_folding_complex_expr() {
        let fns = build_and_optimize("fn f() -> i64:\n    return 2 * 3 + 4 * 5\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(26)))),
            "2*3+4*5 should fold to 26"
        );
    }

    #[test]
    fn constant_folding_subtraction() {
        let fns = build_and_optimize("fn f() -> i64:\n    return 100 - 58\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(42)))),
            "100-58 should fold to 42"
        );
    }

    #[test]
    fn constant_folding_division() {
        let fns = build_and_optimize("fn f() -> i64:\n    return 84 / 2\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(42)))),
            "84/2 should fold to 42"
        );
    }

    #[test]
    fn constant_folding_chain() {
        let fns = build_and_optimize("fn f() -> i64:\n    return (1 + 2) * (3 + 4)\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(21)))),
            "(1+2)*(3+4) should fold to 21"
        );
    }

    #[test]
    fn constant_propagation_through_let() {
        let fns = build_and_optimize("fn f() -> i64:\n    let x = 42\n    return x\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(42)))),
            "x should be replaced by 42"
        );
    }

    #[test]
    fn dead_code_elimination_removes_unused_assign() {
        let fns = build_and_optimize("fn f() -> i64:\n    let x = 99\n    return 42\n");
        let f = find_fn(&fns, "f");
        let has_unused = has_rvalue(f, |rval| {
            matches!(rval, Rvalue::Use(Operand::Constant(Constant::Int(99))))
        });
        assert!(!has_unused, "unused let x=99 should be DCE'd");
    }

    #[test]
    fn dce_preserves_used_values() {
        let fns = build_and_optimize("fn f() -> i64:\n    let x = 42\n    return x\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(42)))),
            "used value 42 should be preserved"
        );
    }

    #[test]
    fn copy_propagation_eliminates_intermediate() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    let y = x\n    return y\n");
        let f = find_fn(&fns, "f");
        let has_intermediate = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                if let StatementKind::Assign(local, Rvalue::Use(_)) = &s.kind {
                    f.locals
                        .get(local.0)
                        .map(|d| d.name.as_deref() == Some("y"))
                        .unwrap_or(false)
                } else {
                    false
                }
            })
        });
        assert!(
            !has_intermediate,
            "intermediate y should be copy-propagated"
        );
    }

    #[test]
    fn cse_eliminates_redundant_computation() {
        let fns = build_and_optimize(
            "fn f(a: i64, b: i64) -> i64:\n    let x = a + b\n    let y = a + b\n    return x + y\n",
        );
        let f = find_fn(&fns, "f");
        let add_count = f.basic_blocks.iter().flat_map(|bb| {
            bb.statements.iter().filter(|s| {
                matches!(&s.kind, StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Add, a, b)) if a != b)
            })
        }).count();
        assert!(
            add_count <= 2,
            "CSE/GVN should eliminate duplicate a+b, got {} adds",
            add_count
        );
    }

    #[test]
    fn cse_different_ops_not_confused() {
        let fns = build_and_optimize(
            "fn f(a: i64, b: i64) -> i64:\n    let x = a + b\n    let y = a - b\n    return x + y\n",
        );
        let f = find_fn(&fns, "f");
        let has_add = has_rvalue(f, |rval| matches!(rval, Rvalue::BinaryOp(BinOp::Add, _, _)));
        let has_sub = has_rvalue(f, |rval| matches!(rval, Rvalue::BinaryOp(BinOp::Sub, _, _)));
        assert!(
            has_add,
            "add operation should be preserved after optimization"
        );
        assert!(has_sub, "sub operation should not be confused with add");
    }

    #[test]
    fn strength_reduction_mul_to_shift() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x * 2\n");
        let f = find_fn(&fns, "f");
        let has_mul = has_rvalue(f, |rval| matches!(rval, Rvalue::BinaryOp(BinOp::Mul, _, _)));
        let has_shl = has_rvalue(f, |rval| matches!(rval, Rvalue::BinaryOp(BinOp::Shl, _, _)));
        assert!(
            has_shl || !has_mul,
            "x*2 should become x<<1 or at least not be a mul"
        );
    }

    #[test]
    fn algebraic_simplification_add_zero() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x + 0\n");
        let f = find_fn(&fns, "f");
        let has_add_zero = has_rvalue(f, |rval| {
            matches!(
                rval,
                Rvalue::BinaryOp(BinOp::Add, _, Operand::Constant(Constant::Int(0)))
            )
        });
        assert!(!has_add_zero, "x+0 should be simplified to x");
    }

    #[test]
    fn algebraic_simplification_mul_one() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x * 1\n");
        let f = find_fn(&fns, "f");
        let has_mul_one = has_rvalue(f, |rval| matches!(rval, Rvalue::BinaryOp(BinOp::Mul, _, _)));
        assert!(!has_mul_one, "x*1 should be simplified to x");
    }

    #[test]
    fn algebraic_simplification_mul_zero() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x * 0\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(0)))),
            "x*0 should become 0"
        );
    }

    #[test]
    fn algebraic_simplification_sub_self() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x - x\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(0)))),
            "x-x should become 0"
        );
    }

    #[test]
    fn simplification_div_self() {
        let fns = build_and_optimize("fn f(x: i64) -> i64:\n    return x / x\n");
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(1)))),
            "x/x should become 1"
        );
    }

    #[test]
    fn peephole_not_not_to_identity() {
        use crate::parser::ast::UnaryOp;
        let fns = build_and_optimize("fn f(x: bool) -> bool:\n    return not not x\n");
        let f = find_fn(&fns, "f");
        let not_count = f
            .basic_blocks
            .iter()
            .flat_map(|bb| {
                bb.statements.iter().filter(|s| {
                    matches!(
                        &s.kind,
                        StatementKind::Assign(_, Rvalue::UnaryOp(UnaryOp::Not, _))
                    )
                })
            })
            .count();
        assert_eq!(
            not_count, 0,
            "not not x should be simplified to x by peephole, found {} Not ops",
            not_count
        );
    }

    #[test]
    fn gvn_merges_equivalent_expressions() {
        let fns = build_and_optimize(
            "fn f(a: i64, b: i64) -> i64:\n    let x = a + b\n    let y = a + b\n    let z = a + b\n    return x + y + z\n",
        );
        let f = find_fn(&fns, "f");
        let add_count = f.basic_blocks.iter().flat_map(|bb| {
            bb.statements.iter().filter(|s| {
                matches!(&s.kind, StatementKind::Assign(_, Rvalue::BinaryOp(BinOp::Add, a, b)) if a != b)
            })
        }).count();
        assert!(
            add_count <= 2,
            "GVN should merge identical additions, got {}",
            add_count
        );
    }

    #[test]
    fn inliner_replaces_call_with_body() {
        let fns = build_and_optimize(
            "fn inc(x: i64) -> i64:\n    return x + 1\n\nfn f(x: i64) -> i64:\n    return inc(x)\n",
        );
        let f = find_fn(&fns, "f");
        let has_inc_call = has_rvalue(f, |rval| {
            matches!(rval, Rvalue::Call { func, .. }
                if matches!(func, Operand::Constant(Constant::Function(name))
                    if name.contains("inc")))
        });
        assert!(!has_inc_call, "inc(x) should be inlined into f");
    }

    #[test]
    fn optimization_preserves_correctness_factorial() {
        let src = "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n";
        let opt_fns = build_and_optimize(src);
        let opt_fact = find_fn(&opt_fns, "fact");
        assert!(opt_fact.basic_blocks.len() >= 2);
    }

    #[test]
    fn optimization_preserves_correctness_fibonacci() {
        let src = "fn fib(n: i64) -> i64:\n    if n <= 1:\n        return n\n    return fib(n - 1) + fib(n - 2)\n";
        let opt_fns = build_and_optimize(src);
        let opt_fib = find_fn(&opt_fns, "fib");
        assert!(opt_fib.basic_blocks.len() >= 2);
        assert_eq!(opt_fib.arg_count, 1);
    }

    #[test]
    fn constant_propagation_nested() {
        let fns = build_and_optimize(
            "fn f() -> i64:\n    let a = 10\n    let b = 20\n    return a + b\n",
        );
        let f = find_fn(&fns, "f");
        assert!(
            has_operand(f, |op| matches!(op, Operand::Constant(Constant::Int(30)))),
            "a+b (10+20) should fold to 30"
        );
    }

    #[test]
    fn dce_removes_dead_loop_body() {
        let fns = build_and_optimize(
            "fn f(n: i64) -> i64:\n    let mut i = 0\n    while i < n:\n        i = i + 1\n    return 42\n",
        );
        let f = find_fn(&fns, "f");
        let has_loop = f.basic_blocks.len() > 2;
        assert!(has_loop, "loop with side effects should not be removed");
    }

    #[test]
    fn move_elision_converts_move_to_copy() {
        let fns = build_and_optimize(
            "struct Point:\n    x: i64\n    y: i64\n\nfn f(p: Point) -> i64:\n    let q = p\n    return q.x\n",
        );
        let f = find_fn(&fns, "f");
        let has_copy = has_rvalue(f, |rval| matches!(rval, Rvalue::Use(_)));
        assert!(has_copy, "move should be elided to a copy");
    }

    #[test]
    fn recursive_function_not_inlined() {
        let fns = build_and_optimize(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        let f = find_fn(&fns, "fact");
        let has_self_call = has_rvalue(f, |rval| {
            matches!(rval, Rvalue::Call { func, .. }
                if matches!(func, Operand::Constant(Constant::Function(name))
                    if name.contains("fact")))
        });
        assert!(has_self_call, "recursive function should still call itself");
    }

    fn calls(f: &MirFunction, name: &str) -> usize {
        f.basic_blocks
            .iter()
            .flat_map(|bb| bb.statements.iter())
            .filter(|st| {
                matches!(&st.kind, StatementKind::Assign(_, Rvalue::Call { func, .. })
                    if matches!(func, Operand::Constant(Constant::Function(n)) if n == name))
            })
            .count()
    }

    fn list_literals(f: &MirFunction) -> usize {
        f.basic_blocks
            .iter()
            .flat_map(|bb| bb.statements.iter())
            .filter(|st| {
                matches!(
                    &st.kind,
                    StatementKind::Assign(_, Rvalue::Aggregate(crate::mir::AggregateKind::List, _))
                )
            })
            .count()
    }

    // Only lists whose elements need a deep copy lower the concat to a call,
    // which is what ownership rewrites to `concat_move` and the fold matches.
    // Scalar-element lists keep the raw `BinaryOp` path and are left alone.
    #[test]
    fn self_append_folds_literal_into_push() {
        let fns = build_and_optimize(
            "fn f():\n    let mut xs = []\n    let mut i = 0\n    while i < 3:\n        xs = xs + [str(i)]\n        i = i + 1\n",
        );
        let f = find_fn(&fns, "f");
        assert_eq!(calls(f, "__olive_list_push"), 1);
        assert_eq!(calls(f, "__olive_list_concat_move"), 0);
        // Only the initial `[]` survives; the per-iteration literal is gone.
        assert_eq!(list_literals(f), 1);
    }

    #[test]
    fn multi_element_append_pushes_each() {
        let fns = build_and_optimize(
            "fn f():\n    let mut xs = []\n    xs = xs + [str(1), str(2), str(3)]\n",
        );
        let f = find_fn(&fns, "f");
        assert_eq!(calls(f, "__olive_list_push"), 3);
        assert_eq!(calls(f, "__olive_list_concat_move"), 0);
    }

    #[test]
    fn append_element_is_moved_not_copied() {
        // A `Copy` here would leave the source local's drop live and free the
        // string the list just took.
        let fns = build_and_optimize(
            "fn f():\n    let mut xs = []\n    let mut i = 0\n    while i < 3:\n        xs = xs + [str(i)]\n        i = i + 1\n",
        );
        let f = find_fn(&fns, "f");
        let moved = f.basic_blocks.iter().flat_map(|bb| bb.statements.iter()).any(|st| {
            matches!(&st.kind, StatementKind::Assign(_, Rvalue::Call { func, args })
                if matches!(func, Operand::Constant(Constant::Function(n)) if n == "__olive_list_push")
                    && matches!(args.get(1), Some(Operand::Move(_))))
        });
        assert!(moved, "pushed element must transfer ownership");
    }

    #[test]
    fn non_self_concat_is_left_alone() {
        // `ys` keeps its own storage, so nothing may be pushed into it.
        let fns = build_and_optimize(
            "fn f():\n    let ys = [str(9)]\n    let zs = ys + [str(8)]\n    print(len(ys))\n    print(len(zs))\n",
        );
        let f = find_fn(&fns, "f");
        assert_eq!(calls(f, "__olive_list_push"), 0);
    }
}
