//! `evaluate` request: the read-only entry point over `expr.rs`'s
//! arithmetic-expression grammar. A bare path (`xs[1].name`, no operators)
//! resolves through `values::frame_variables`/`values::children` so its
//! result stays expandable in the UI exactly as a plain read would;
//! anything with an operator (or an aggregate literal) evaluates to a
//! plain scalar via `expr::eval_arith` instead.

pub(crate) use super::expr::*;
use super::launch::DebugSession;
use super::values::{self, Variable};

pub fn evaluate(session: &DebugSession, frame_idx: usize, expr: &str) -> Result<Variable, String> {
    let ast = parse_arith(expr)?;
    if let AExpr::Path(tokens) = &ast {
        return evaluate_path(session, frame_idx, tokens);
    }

    let Some((fn_id, cells)) = session.frame_cells(frame_idx) else {
        return Err("not stopped".to_string());
    };
    let value = eval_arith(session, fn_id, &cells, &ast)?;
    Ok(Variable {
        name: expr.to_string(),
        type_name: value.type_name().to_string(),
        value: value.to_string(),
        reference: 0,
    })
}

fn evaluate_path(
    session: &DebugSession,
    frame_idx: usize,
    tokens: &[Token],
) -> Result<Variable, String> {
    let Some(Token::Ident(root)) = tokens.first() else {
        return Err("empty path".to_string());
    };

    let vars = values::frame_variables(session, frame_idx);
    let mut current = vars
        .into_iter()
        .find(|v| v.name == *root)
        .ok_or_else(|| format!("no such variable: {root}"))?;

    for tok in &tokens[1..] {
        if current.reference == 0 {
            return Err(format!("{} has no fields or elements", current.name));
        }
        let key = token_key(tok);
        current = values::children(session, current.reference)
            .into_iter()
            .find(|c| c.name == key)
            .ok_or_else(|| format!("no such member: {key}"))?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::dap::launch::launch;

    const SRC: &str = "struct Point:\n    x: int\n    y: int\n    fn __init__(self, x: int, y: int):\n        self.x = x\n        self.y = y\n\nfn main():\n    let p = Point(1, 2)\n    let xs = [10, 20, 30]\n    let d = {\"a\": 1, \"b\": 2}\n    let n = 7\n    print(p)\n    print(xs)\n    print(d)\n    print(n)\n";

    /// The exec lock guards the process-wide "one session at a time" slot
    /// too, so it must stay held for the caller's whole test, not just
    /// through setup -- returning it alongside the session keeps it alive.
    fn stopped_session() -> (DebugSession, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::test_utils::exec_lock();
        let thread_name: String = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let path = std::env::temp_dir().join(format!(
            "olive_eval_test_{}_{}.liv",
            std::process::id(),
            thread_name
        ));
        std::fs::write(&path, SRC).unwrap();
        let session = launch(path.to_str().unwrap(), false).expect("launch failed");
        session.set_breakpoints(0, &[13]);
        session.cont(1);
        session.events().recv().unwrap();
        std::fs::remove_file(&path).ok();
        (session, guard)
    }

    #[test]
    fn plain_ident_resolves_a_local() {
        let (session, _guard) = stopped_session();
        let v = evaluate(&session, 0, "xs").unwrap();
        assert_eq!(v.value, "[10, 20, 30]");
    }

    #[test]
    fn list_index_and_struct_field_resolve() {
        let (session, _guard) = stopped_session();
        assert_eq!(evaluate(&session, 0, "xs[1]").unwrap().value, "20");
        assert_eq!(evaluate(&session, 0, "p.x").unwrap().value, "1");
    }

    #[test]
    fn dict_string_key_resolves() {
        let (session, _guard) = stopped_session();
        assert_eq!(evaluate(&session, 0, "d[\"a\"]").unwrap().value, "1");
    }

    #[test]
    fn unknown_root_is_an_error_not_a_panic() {
        let (session, _guard) = stopped_session();
        assert!(evaluate(&session, 0, "nope").is_err());
    }

    #[test]
    fn malformed_expression_is_an_error() {
        let (session, _guard) = stopped_session();
        assert!(evaluate(&session, 0, "xs[").is_err());
        assert!(evaluate(&session, 0, "9xs").is_err());
    }

    #[test]
    fn arithmetic_over_paths_and_literals() {
        let (session, _guard) = stopped_session();
        assert_eq!(evaluate(&session, 0, "n + 1").unwrap().value, "8");
        assert_eq!(evaluate(&session, 0, "n * 2 - 1").unwrap().value, "13");
        assert_eq!(evaluate(&session, 0, "(n + 1) * 2").unwrap().value, "16");
        assert_eq!(evaluate(&session, 0, "n / 2").unwrap().value, "3");
        assert_eq!(evaluate(&session, 0, "n % 2").unwrap().value, "1");
        assert_eq!(evaluate(&session, 0, "-n").unwrap().value, "-7");
        assert_eq!(evaluate(&session, 0, "xs[1] + xs[0]").unwrap().value, "30");
        assert_eq!(evaluate(&session, 0, "1 + 2.5").unwrap().value, "3.5");
    }

    #[test]
    fn arithmetic_result_has_no_reference() {
        let (session, _guard) = stopped_session();
        assert_eq!(evaluate(&session, 0, "n + 1").unwrap().reference, 0);
    }

    #[test]
    fn division_by_zero_is_an_error_not_a_crash() {
        let (session, _guard) = stopped_session();
        assert!(evaluate(&session, 0, "n / 0").is_err());
        assert!(evaluate(&session, 0, "n % 0").is_err());
    }
}
