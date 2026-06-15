//! Conditional breakpoints, hit counts, and logpoints. Parsed once when a
//! breakpoint is set (`engine::EngineShared::set_breakpoints_with`), then
//! evaluated inside the stmt hook's stop path, only after the packed-line
//! hash has already matched a set breakpoint. Path operands reuse `eval.rs`'s
//! tokenizer; descending through a container reuses `values.rs`'s type
//! dispatch. Neither needs a parked session: the frame handed in here is the
//! raw snapshot the hook just captured, read before any stop decision.

use super::engine::EngineShared;
use super::eval::{self, Token};
use super::hooks::Frame;
use super::values;
use crate::semantic::type_descriptor::concrete_ty;
use crate::semantic::types::Type;

pub(crate) enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp(Operand, CmpOp, Operand),
    Truthy(Operand),
}

pub(crate) enum Operand {
    Path(Vec<Token>),
    Int(i64),
    Bool(bool),
    Str(String),
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

enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => write!(f, "{s}"),
        }
    }
}

pub(crate) enum LogPart {
    Text(String),
    Expr(Vec<Token>),
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
        chars: src.chars().collect(),
        pos: 0,
    };
    let e = p.parse_or()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
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
                parts.push(LogPart::Expr(eval::tokenize(inner.trim())?));
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
            LogPart::Expr(tokens) => match resolve_tokens(shared, top, tokens) {
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
            let av = resolve_operand(shared, top, a)?;
            let bv = resolve_operand(shared, top, b)?;
            compare(*op, av, bv)
        }
        Expr::Truthy(a) => match resolve_operand(shared, top, a)? {
            Value::Bool(b) => Ok(b),
            other => Err(format!("expected a bool condition, found {other}")),
        },
    }
}

fn resolve_operand(shared: &EngineShared, top: &Frame, op: &Operand) -> Result<Value, String> {
    match op {
        Operand::Int(n) => Ok(Value::Int(*n)),
        Operand::Bool(b) => Ok(Value::Bool(*b)),
        Operand::Str(s) => Ok(Value::Str(s.clone())),
        Operand::Path(tokens) => resolve_tokens(shared, top, tokens),
    }
}

/// Root + descent for a path operand, against the frame the hook just
/// captured rather than the "parked" state `eval.rs`'s `evaluate` reads --
/// this runs before any stop decision, so nothing has parked yet.
fn resolve_tokens(shared: &EngineShared, top: &Frame, tokens: &[Token]) -> Result<Value, String> {
    let Some(Token::Ident(root)) = tokens.first() else {
        return Err("empty path".to_string());
    };
    let cells = shared.fn_cells(top.fn_id);
    let idx = cells
        .iter()
        .position(|c| c.name == *root)
        .ok_or_else(|| format!("no such variable: {root}"))?;
    let mut raw = top.cells.get(idx).copied().unwrap_or(0);
    let mut ty = cells[idx].ty.clone();

    for tok in &tokens[1..] {
        let key = match tok {
            Token::Ident(name) => name.clone(),
            Token::Index(i) => i.to_string(),
            Token::Key(s) => format!("\"{s}\""),
        };
        let (_, cval, cty) = values::children_raw(shared, raw, &ty)
            .into_iter()
            .find(|(n, _, _)| *n == key)
            .ok_or_else(|| format!("no such member: {key}"))?;
        raw = cval;
        ty = cty;
    }
    decode_value(shared, raw, &ty)
}

fn decode_value(shared: &EngineShared, raw: i64, ty: &Type) -> Result<Value, String> {
    match concrete_ty(ty) {
        Type::Bool => Ok(Value::Bool(raw != 0)),
        Type::Str => Ok(Value::Str(values::str_value(shared, raw))),
        Type::F32 => Ok(Value::Float(f32::from_bits(raw as u32) as f64)),
        Type::Float => Ok(Value::Float(f64::from_bits(raw as u64))),
        Type::Int
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize => Ok(Value::Int(raw)),
        other => Err(format!("cannot use a {other} value in a condition")),
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

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn skip_ws(&mut self) {
        while matches!(self.chars.get(self.pos), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn peek_word(&mut self, word: &str) -> bool {
        self.skip_ws();
        let w: Vec<char> = word.chars().collect();
        if self.pos + w.len() > self.chars.len() {
            return false;
        }
        if self.chars[self.pos..self.pos + w.len()] != w[..] {
            return false;
        }
        self.chars
            .get(self.pos + w.len())
            .is_none_or(|&c| !eval::is_ident_char(c))
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.peek_word(word) {
            self.pos += word.chars().count();
            true
        } else {
            false
        }
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_and()?;
        while self.eat_word("or") {
            lhs = Expr::Or(Box::new(lhs), Box::new(self.parse_and()?));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_not()?;
        while self.eat_word("and") {
            lhs = Expr::And(Box::new(lhs), Box::new(self.parse_not()?));
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.eat_word("not") {
            return Ok(Expr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let lhs = self.parse_operand()?;
        self.skip_ws();
        match self.try_cmp_op() {
            Some(op) => Ok(Expr::Cmp(lhs, op, self.parse_operand()?)),
            None => Ok(Expr::Truthy(lhs)),
        }
    }

    fn try_cmp_op(&mut self) -> Option<CmpOp> {
        for (text, op) in CMP_OPS {
            let t: Vec<char> = text.chars().collect();
            if self.chars[self.pos..].starts_with(&t[..]) {
                self.pos += t.len();
                return Some(op);
            }
        }
        None
    }

    fn parse_operand(&mut self) -> Result<Operand, String> {
        self.skip_ws();
        match self.chars.get(self.pos) {
            Some('"') => self.parse_string_literal().map(Operand::Str),
            Some(c) if c.is_ascii_digit() => self.parse_int_literal().map(Operand::Int),
            Some('-')
                if self
                    .chars
                    .get(self.pos + 1)
                    .is_some_and(char::is_ascii_digit) =>
            {
                self.parse_int_literal().map(Operand::Int)
            }
            Some(&c) if eval::is_ident_char(c) => {
                if self.eat_word("true") {
                    return Ok(Operand::Bool(true));
                }
                if self.eat_word("false") {
                    return Ok(Operand::Bool(false));
                }
                self.parse_path().map(Operand::Path)
            }
            _ => Err(format!(
                "unexpected input at position {} in condition",
                self.pos
            )),
        }
    }

    fn parse_int_literal(&mut self) -> Result<i64, String> {
        let start = self.pos;
        if self.chars.get(self.pos) == Some(&'-') {
            self.pos += 1;
        }
        while matches!(self.chars.get(self.pos), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse()
            .map_err(|_| format!("invalid integer literal '{text}'"))
    }

    fn parse_string_literal(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.chars.get(self.pos) {
                None => return Err("unterminated string literal".to_string()),
                Some('"') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') if self.chars.get(self.pos + 1).is_some() => {
                    s.push(self.chars[self.pos + 1]);
                    self.pos += 2;
                }
                Some(&c) => {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
        Ok(s)
    }

    /// Scans an `ident ('.' ident | '[' ... ']')*` span out of the larger
    /// expression, then hands it to `eval::tokenize` so the tokens it
    /// produces are identical to the `evaluate` request's.
    fn parse_path(&mut self) -> Result<Vec<Token>, String> {
        let start = self.pos;
        while matches!(self.chars.get(self.pos), Some(&c) if eval::is_ident_char(c)) {
            self.pos += 1;
        }
        loop {
            match self.chars.get(self.pos) {
                Some('.') => {
                    self.pos += 1;
                    while matches!(self.chars.get(self.pos), Some(&c) if eval::is_ident_char(c)) {
                        self.pos += 1;
                    }
                }
                Some('[') => {
                    self.pos += 1;
                    let mut in_str = false;
                    while let Some(&c) = self.chars.get(self.pos) {
                        self.pos += 1;
                        match c {
                            '\\' if in_str => self.pos += 1,
                            '"' => in_str = !in_str,
                            ']' if !in_str => break,
                            _ => {}
                        }
                    }
                }
                _ => break,
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        eval::tokenize(&text)
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
}
