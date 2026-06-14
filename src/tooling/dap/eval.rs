//! `xs[1].name`-style path evaluator for the `evaluate` request. Grammar:
//! `ident ('.' ident | '[' int ']' | '[' quoted_str ']')*`. Resolution
//! reuses `values::frame_variables`/`values::children` (the D4 traversal)
//! rather than re-reading heap layout here; errors are messages, never
//! panics. No arithmetic, no calls: paths only.

use super::launch::DebugSession;
use super::values::{self, Variable};

pub fn evaluate(session: &DebugSession, frame_idx: usize, expr: &str) -> Result<Variable, String> {
    let mut tokens = tokenize(expr)?.into_iter();
    let Some(Token::Ident(root)) = tokens.next() else {
        return Err(format!("'{expr}' is not a valid expression"));
    };

    let vars = values::frame_variables(session, frame_idx);
    let mut current = vars
        .into_iter()
        .find(|v| v.name == root)
        .ok_or_else(|| format!("no such variable: {root}"))?;

    for tok in tokens {
        if current.reference == 0 {
            return Err(format!("{} has no fields or elements", current.name));
        }
        let key = match &tok {
            Token::Ident(name) => name.clone(),
            Token::Index(i) => i.to_string(),
            Token::Key(s) => format!("\"{s}\""),
        };
        current = values::children(session, current.reference)
            .into_iter()
            .find(|c| c.name == key)
            .ok_or_else(|| format!("no such member: {key}"))?;
    }
    Ok(current)
}

enum Token {
    Ident(String),
    Index(i64),
    Key(String),
}

fn tokenize(expr: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();

    let start = i;
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    if i == start {
        return Err(format!("'{expr}' does not start with an identifier"));
    }
    out.push(Token::Ident(chars[start..i].iter().collect()));

    while i < chars.len() {
        match chars[i] {
            '.' => {
                i += 1;
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                if i == start {
                    return Err(format!("expected a field name after '.' in '{expr}'"));
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            }
            '[' => {
                i += 1;
                if chars.get(i) == Some(&'"') {
                    i += 1;
                    let mut s = String::new();
                    loop {
                        match chars.get(i) {
                            None => return Err(format!("unterminated string in '{expr}'")),
                            Some('"') => break,
                            Some('\\') if chars.get(i + 1).is_some() => {
                                s.push(chars[i + 1]);
                                i += 2;
                            }
                            Some(&c) => {
                                s.push(c);
                                i += 1;
                            }
                        }
                    }
                    i += 1; // closing quote
                    if chars.get(i) != Some(&']') {
                        return Err(format!("expected ']' after string index in '{expr}'"));
                    }
                    i += 1;
                    out.push(Token::Key(s));
                } else {
                    let start = i;
                    if chars.get(i) == Some(&'-') {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        return Err(format!("expected an integer index in '{expr}'"));
                    }
                    let text: String = chars[start..i].iter().collect();
                    let n: i64 = text
                        .parse()
                        .map_err(|_| format!("invalid integer index in '{expr}'"))?;
                    if chars.get(i) != Some(&']') {
                        return Err(format!("expected ']' after index in '{expr}'"));
                    }
                    i += 1;
                    out.push(Token::Index(n));
                }
            }
            _ => return Err(format!("unexpected character in '{expr}' at position {i}")),
        }
    }
    Ok(out)
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::dap::launch::launch;

    const SRC: &str = "struct Point:\n    x: int\n    y: int\n    fn __init__(self, x: int, y: int):\n        self.x = x\n        self.y = y\n\nfn main():\n    let p = Point(1, 2)\n    let xs = [10, 20, 30]\n    let d = {\"a\": 1, \"b\": 2}\n    print(p)\n    print(xs)\n    print(d)\n";

    /// The exec lock guards the process-wide "one session at a time" slot
    /// too, so it must stay held for the caller's whole test, not just
    /// through setup -- returning it alongside the session keeps it alive.
    fn stopped_session() -> (DebugSession, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::test_utils::exec_lock();
        let path = std::env::temp_dir().join(format!(
            "olive_eval_test_{}_{}.liv",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        std::fs::write(&path, SRC).unwrap();
        let session = launch(path.to_str().unwrap(), false).expect("launch failed");
        session.set_breakpoints(0, &[12]);
        session.cont();
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
}
