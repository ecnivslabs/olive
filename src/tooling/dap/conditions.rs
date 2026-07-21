//! Conditional breakpoints, hit counts, and logpoints. Parsed once when a
//! breakpoint is set (`engine::EngineShared::set_breakpoints_with`), then
//! evaluated inside the stmt hook's stop path, only after the packed-line
//! hash has already matched a set breakpoint. Operands are `eval.rs`'s
//! arithmetic expressions (paths, literals, `+ - * / %`, parens); this
//! module adds only the boolean layer (`and`/`or`/`not`/comparisons) on
//! top. Neither needs a parked session: the frame handed in here is the
//! raw snapshot the hook just captured, read before any stop decision.

use super::engine::EngineShared;
use super::eval::{self, AExpr, ArithParser, Value};
use super::hooks::Frame;

pub(crate) enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp(AExpr, CmpOp, AExpr),
    Truthy(AExpr),
}

#[derive(Clone, Copy)]
pub(crate) enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

const CMP_OPS: [(&str, CmpOp); 6] = [
    ("==", CmpOp::Eq),
    ("!=", CmpOp::Ne),
    ("<=", CmpOp::Le),
    (">=", CmpOp::Ge),
    ("<", CmpOp::Lt),
    (">", CmpOp::Gt),
];

pub(crate) enum LogPart {
    Text(String),
    Expr(AExpr),
}

pub(crate) type LogTemplate = Vec<LogPart>;

#[derive(Clone, Copy)]
pub(crate) enum HitCondition {
    Exact(u64),
    Mod(u64),
    Cmp(CmpOp, u64),
}

impl HitCondition {
    pub(crate) fn matches(&self, n: u64) -> bool {
        match self {
            HitCondition::Exact(k) => n == *k,
            HitCondition::Mod(k) => n.is_multiple_of(*k),
            HitCondition::Cmp(op, k) => apply(*op, n, *k),
        }
    }
}

pub(crate) fn parse_hit_condition(s: &str) -> Result<HitCondition, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('%') {
        return rest
            .trim()
            .parse()
            .map(HitCondition::Mod)
            .map_err(|_| format!("invalid hit count '{s}'"));
    }
    for (prefix, op) in CMP_OPS {
        if let Some(rest) = s.strip_prefix(prefix) {
            let n: u64 = rest
                .trim()
                .parse()
                .map_err(|_| format!("invalid hit count '{s}'"))?;
            return Ok(HitCondition::Cmp(op, n));
        }
    }
    s.parse()
        .map(HitCondition::Exact)
        .map_err(|_| format!("invalid hit count '{s}'"))
}

pub(crate) fn parse_condition(src: &str) -> Result<Expr, String> {
    let mut p = Parser {
        arith: ArithParser::new(src),
    };
    let e = p.parse_or()?;
    if !p.arith.at_end() {
        return Err(format!("unexpected trailing input in condition '{src}'"));
    }
    Ok(e)
}

pub(crate) fn parse_log_template(template: &str) -> Result<LogTemplate, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if !text.is_empty() {
                    parts.push(LogPart::Text(std::mem::take(&mut text)));
                }
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(format!("unterminated '{{' in log message '{template}'"));
                }
                let inner: String = chars[start..i].iter().collect();
                parts.push(LogPart::Expr(eval::parse_arith(inner.trim())?));
                i += 1;
            }
            '}' => return Err(format!("unmatched '}}' in log message '{template}'")),
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    if !text.is_empty() {
        parts.push(LogPart::Text(text));
    }
    Ok(parts)
}

pub(crate) fn render_log(shared: &EngineShared, top: &Frame, template: &LogTemplate) -> String {
    let mut out = String::new();
    for part in template {
        match part {
            LogPart::Text(t) => out.push_str(t),
            LogPart::Expr(expr) => match eval::eval_arith(shared, top.fn_id, &top.cells, expr) {
                Ok(v) => out.push_str(&v.to_string()),
                Err(e) => out.push_str(&format!("<{e}>")),
            },
        }
    }
    out.push('\n');
    out
}

pub(crate) fn eval_expr(shared: &EngineShared, top: &Frame, expr: &Expr) -> Result<bool, String> {
    match expr {
        Expr::Or(l, r) => Ok(eval_expr(shared, top, l)? || eval_expr(shared, top, r)?),
        Expr::And(l, r) => Ok(eval_expr(shared, top, l)? && eval_expr(shared, top, r)?),
        Expr::Not(e) => Ok(!eval_expr(shared, top, e)?),
        Expr::Cmp(a, op, b) => {
            let av = eval::eval_arith(shared, top.fn_id, &top.cells, a)?;
            let bv = eval::eval_arith(shared, top.fn_id, &top.cells, b)?;
            compare(*op, av, bv)
        }
        Expr::Truthy(a) => match eval::eval_arith(shared, top.fn_id, &top.cells, a)? {
            Value::Bool(b) => Ok(b),
            other => Err(format!("expected a bool condition, found {other}")),
        },
    }
}

fn compare(op: CmpOp, a: Value, b: Value) -> Result<bool, String> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(apply(op, x, y)),
        (Value::Float(x), Value::Float(y)) => Ok(apply(op, x, y)),
        (Value::Int(x), Value::Float(y)) => Ok(apply(op, x as f64, y)),
        (Value::Float(x), Value::Int(y)) => Ok(apply(op, x, y as f64)),
        (Value::Bool(x), Value::Bool(y)) => Ok(apply(op, x, y)),
        (Value::Str(x), Value::Str(y)) => Ok(apply(op, x, y)),
        (a, b) => Err(format!("cannot compare {a} and {b}")),
    }
}

fn apply<T: PartialOrd>(op: CmpOp, a: T, b: T) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

/// The boolean layer (`or`/`and`/`not`/comparisons) wrapped around
/// `eval.rs`'s arithmetic-expression parser, sharing its character
/// position so both grammars scan one continuous input.
struct Parser {
    arith: ArithParser,
}

impl Parser {
    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_and()?;
        while self.arith.eat_word("or") {
            lhs = Expr::Or(Box::new(lhs), Box::new(self.parse_and()?));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_not()?;
        while self.arith.eat_word("and") {
            lhs = Expr::And(Box::new(lhs), Box::new(self.parse_not()?));
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.arith.eat_word("not") {
            return Ok(Expr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let lhs = self.arith.parse_add_sub()?;
        self.arith.skip_ws();
        match self.try_cmp_op() {
            Some(op) => Ok(Expr::Cmp(lhs, op, self.arith.parse_add_sub()?)),
            None => Ok(Expr::Truthy(lhs)),
        }
    }

    fn try_cmp_op(&mut self) -> Option<CmpOp> {
        for (text, op) in CMP_OPS {
            if self.arith.remaining_starts_with(text) {
                self.arith.advance(text.chars().count());
                return Some(op);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comparisons_and_logical_operators() {
        assert!(parse_condition("i == 500").is_ok());
        assert!(parse_condition("i != 500 and flag").is_ok());
        assert!(parse_condition("not flag or i < 10").is_ok());
        assert!(parse_condition("name == \"bob\"").is_ok());
        assert!(parse_condition("xs[0].x >= 5").is_ok());
    }

    #[test]
    fn parses_arithmetic_in_operands() {
        assert!(parse_condition("i + 1 == 500").is_ok());
        assert!(parse_condition("i == count * 2 - 1").is_ok());
        assert!(parse_condition("(i + 1) % 2 == 0").is_ok());
    }

    #[test]
    fn rejects_malformed_conditions() {
        assert!(parse_condition("i ==").is_err());
        assert!(parse_condition("i === 5").is_err());
        assert!(parse_condition("").is_err());
        assert!(parse_condition("i == 5 and").is_err());
    }

    #[test]
    fn hit_condition_forms_parse_and_match() {
        assert!(parse_hit_condition("%10").unwrap().matches(10));
        assert!(!parse_hit_condition("%10").unwrap().matches(5));
        assert!(parse_hit_condition(">=5").unwrap().matches(5));
        assert!(!parse_hit_condition(">=5").unwrap().matches(4));
        assert!(parse_hit_condition("5").unwrap().matches(5));
        assert!(!parse_hit_condition("5").unwrap().matches(6));
        assert!(parse_hit_condition("bogus").is_err());
    }

    #[test]
    fn log_template_splits_text_and_expressions() {
        let t = parse_log_template("i is {i}, done").unwrap();
        assert_eq!(t.len(), 3);
        assert!(parse_log_template("unterminated {i").is_err());
    }

    #[test]
    fn log_template_expr_accepts_arithmetic() {
        assert!(parse_log_template("i+1 is {i + 1}").is_ok());
    }
}
