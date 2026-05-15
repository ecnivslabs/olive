use super::super::CraneliftCodegen;
use cranelift_module::{DataDescription, Linkage, Module};

/// Name of the pointer-sized dispatch cell holding a function's current entry point.
fn dispatch_name(func_name: &str) -> String {
    format!("__olive_dispatch${func_name}")
}

impl<M: Module> CraneliftCodegen<M> {
    /// One dispatch cell per non-async function with a specializable `Any`
    /// site (`count_any_add_sites`) -- narrower than "contains a loop" since
    /// `retier` no longer re-optimizes, so specialization is the only payoff.
    /// A loop with no such site gets no cell: pure indirection tax otherwise.
    pub(super) fn generate_dispatch_cells(&mut self) {
        if !self.profile {
            return;
        }
        let (_, any_add_ranges) = self.count_any_add_sites();
        let names: Vec<String> = self
            .functions
            .iter()
            .filter(|f| {
                !f.is_async
                    && any_add_ranges
                        .get(&f.name)
                        .is_some_and(|&(start, end)| end > start)
            })
            .map(|f| f.name.clone())
            .collect();
        for name in names {
            let Some(&func_id) = self.func_ids.get(&name) else {
                continue;
            };
            let mut data_ctx = DataDescription::new();
            let bytes = vec![0u8; 8];
            data_ctx.define(bytes.into_boxed_slice());
            let local_func = self.module.declare_func_in_data(func_id, &mut data_ctx);
            data_ctx.write_function_addr(0, local_func);

            let id = self
                .module
                .declare_data(&dispatch_name(&name), Linkage::Local, true, false)
                .unwrap();
            self.module.define_data(id, &data_ctx).unwrap();
            self.dispatch_ids.insert(name, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{call_i64_1, call_i64_2, compile};

    #[test]
    fn dispatch_cell_call_still_correct() {
        let mut cg = compile(
            "fn add(a: i64, b: i64) -> i64:\n    return a + b\n\nfn f(x: i64) -> i64:\n    return add(x, add(x, x))\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 14), 42);
    }

    #[test]
    fn dispatch_cell_recursive_call_still_correct() {
        let mut cg = compile(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        assert_eq!(call_i64_1(&mut cg, "fact", 5), 120);
    }

    #[test]
    fn dispatch_cell_mutual_functions_still_correct() {
        let mut cg = compile(
            "fn inc(x: i64) -> i64:\n    return x + 1\n\nfn dec(x: i64) -> i64:\n    return x - 1\n\nfn f(x: i64) -> i64:\n    return inc(dec(x))\n",
        );
        assert_eq!(call_i64_1(&mut cg, "f", 42), 42);
    }

    #[test]
    fn dispatch_cell_multi_arg_still_correct() {
        let mut cg = compile("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert_eq!(call_i64_2(&mut cg, "add", 10, 32), 42);
    }

    #[test]
    fn dispatch_cell_used_for_function_with_any_add_site() {
        let mut cg = compile(concat!(
            "fn sum_to(n: i64) -> Any:\n",
            "    let mut total: Any = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        let a: Any = i\n",
            "        total = total + a\n",
            "        i = i + 1\n",
            "    return total\n",
            "\n",
            "fn f(x: i64) -> i64:\n",
            "    return int(sum_to(x))\n",
        ));
        assert!(cg.dispatch_ids.contains_key("sum_to"));
        assert_eq!(call_i64_1(&mut cg, "f", 100), (0..100).sum::<i64>());
    }

    /// Loop, no `Any` sites -- nothing to specialize, so no cell.
    #[test]
    fn dispatch_cell_skipped_for_loop_with_no_any_add_sites() {
        let mut cg = compile(concat!(
            "fn sum_to(n: i64) -> i64:\n",
            "    let mut total = 0\n",
            "    let mut i = 0\n",
            "    while i < n:\n",
            "        total = total + i\n",
            "        i = i + 1\n",
            "    return total\n",
            "\n",
            "fn f(x: i64) -> i64:\n",
            "    return sum_to(x)\n",
        ));
        assert!(!cg.dispatch_ids.contains_key("sum_to"));
        assert_eq!(call_i64_1(&mut cg, "f", 100), (0..100).sum::<i64>());
    }

    #[test]
    fn dispatch_cell_skipped_for_loopless_recursive_function() {
        let mut cg = compile(
            "fn fib(n: i64) -> i64:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n",
        );
        assert!(!cg.dispatch_ids.contains_key("fib"));
        assert_eq!(call_i64_1(&mut cg, "fib", 10), 55);
    }
}
