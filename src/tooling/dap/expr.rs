//! Path grammar (`ident ('.' ident | '[' int ']' | '[' quoted_str ']')*`) and
//! the arithmetic-expression parser/evaluator built on top of it: `+ - * /`
//! and `%` over paths and int/float/bool/string literals, unary `-`, parens,
//! and the aggregate-literal forms `setvar.rs` builds a whole value from --
//! `[a, b]`, `(a, b)`, `{k: v}`, `Name(args)` -- the same syntax olive source
//! itself uses, no debugger-only grammar. No calls: invoking a function
//! while parked would mutate the debuggee's state mid-inspection, a step too
//! far from what a read-only expression should be able to do.
//!
//! `eval.rs`'s `evaluate` is the read-only entry point over this; `setvar.rs`
//! builds fresh heap values from it; `conditions.rs` layers boolean
//! `and`/`or`/`not`/comparisons on top, reusing `ArithParser` for the shared
//! character position both grammars scan.

use super::engine::EngineShared;
use super::values;
use crate::semantic::type_descriptor::concrete_ty;
use crate::semantic::types::Type;

/// The name a token matches against in `values::children_raw`'s output:
/// a field name verbatim, a sequence/enum index as its decimal string, or a
/// dict key rendered the same quoted way `values.rs` renders a string key.
/// Shared by `eval.rs`'s path descent, `conditions.rs`'s path operands, and
/// `setvar.rs`'s lvalue resolution -- all three walk the same grammar
/// against the same child listing.
pub(crate) fn token_key(tok: &Token) -> String {
    match tok {
        Token::Ident(name) => name.clone(),
        Token::Index(i) => i.to_string(),
        Token::Key(s) => format!("\"{s}\""),
    }
}

/// A path's tokens: `ident`, then zero or more `.field`/`[index]`/`["key"]`.
pub(crate) enum Token {
    Ident(String),
    Index(i64),
    Key(String),
}

pub(crate) fn tokenize(expr: &str) -> Result<Vec<Token>, String> {
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

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Scans an `ident ('.' ident | '[' ... ']')*` span out of a larger
/// expression starting at `*pos`, advances `*pos` past it, and tokenizes
/// the span the same way a bare path argument would. Shared by this
/// module's own path atoms and `conditions.rs`'s path operands, so both
/// grammars walk identical spans.
pub(crate) fn scan_and_tokenize_path(
    chars: &[char],
    pos: &mut usize,
) -> Result<Vec<Token>, String> {
    let start = *pos;
    while matches!(chars.get(*pos), Some(&c) if is_ident_char(c)) {
        *pos += 1;
    }
    loop {
        match chars.get(*pos) {
            Some('.') => {
                *pos += 1;
                while matches!(chars.get(*pos), Some(&c) if is_ident_char(c)) {
                    *pos += 1;
                }
            }
            Some('[') => {
                *pos += 1;
                let mut in_str = false;
                while let Some(&c) = chars.get(*pos) {
                    *pos += 1;
                    match c {
                        '\\' if in_str => *pos += 1,
                        '"' => in_str = !in_str,
                        ']' if !in_str => break,
                        _ => {}
                    }
                }
            }
            _ => break,
        }
    }
    let text: String = chars[start..*pos].iter().collect();
    tokenize(&text)
}

/// A decoded scalar, the result of resolving any leaf operand (a path or a
/// literal) in an arithmetic expression or a condition.
pub(crate) enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Value {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
        }
    }
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

/// An arithmetic expression: paths, literals, unary `-`, and `+ - * / %`
/// over them, left-associative with the usual precedence, plus the
/// aggregate-literal forms `setvar.rs` builds a whole new value from --
/// `[a, b]`, `(a, b)`, `{k: v}`, and `Name(args)` (a struct or enum-variant
/// constructor, resolved against `struct_fields()`/`enum_defs()` at build
/// time, never an arbitrary function call). `conditions.rs` builds its
/// boolean layer (`and`/`or`/`not`/comparisons) on top of this.
pub(crate) enum AExpr {
    Path(Vec<Token>),
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Neg(Box<AExpr>),
    Bin(Box<AExpr>, ArithOp, Box<AExpr>),
    List(Vec<AExpr>),
    Tuple(Vec<AExpr>),
    Dict(Vec<(AExpr, AExpr)>),
    Construct(String, Vec<AExpr>),
}

#[derive(Clone, Copy)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

pub(crate) fn parse_arith(src: &str) -> Result<AExpr, String> {
    let mut p = ArithParser {
        chars: src.chars().collect(),
        pos: 0,
    };
    let e = p.parse_add_sub()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return Err(format!("unexpected trailing input in expression '{src}'"));
    }
    Ok(e)
}

pub(crate) struct ArithParser {
    chars: Vec<char>,
    pos: usize,
}

impl ArithParser {
    pub(crate) fn new(src: &str) -> Self {
        ArithParser {
            chars: src.chars().collect(),
            pos: 0,
        }
    }

    pub(crate) fn skip_ws(&mut self) {
        while matches!(self.chars.get(self.pos), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    pub(crate) fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.pos == self.chars.len()
    }

    /// Whether the unconsumed input starts with `s`, without advancing --
    /// `conditions.rs` uses this to scan for a comparison operator, a
    /// condition-only concept this module doesn't need to know about.
    pub(crate) fn remaining_starts_with(&self, s: &str) -> bool {
        let w: Vec<char> = s.chars().collect();
        self.pos + w.len() <= self.chars.len() && self.chars[self.pos..self.pos + w.len()] == w[..]
    }

    pub(crate) fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    pub(crate) fn eat_word(&mut self, word: &str) -> bool {
        self.skip_ws();
        let w: Vec<char> = word.chars().collect();
        if self.pos + w.len() > self.chars.len()
            || self.chars[self.pos..self.pos + w.len()] != w[..]
        {
            return false;
        }
        if self
            .chars
            .get(self.pos + w.len())
            .is_some_and(|&c| is_ident_char(c))
        {
            return false;
        }
        self.pos += w.len();
        true
    }

    pub(crate) fn parse_add_sub(&mut self) -> Result<AExpr, String> {
        let mut lhs = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            let op = match self.chars.get(self.pos) {
                Some('+') => ArithOp::Add,
                Some('-') => ArithOp::Sub,
                _ => break,
            };
            self.pos += 1;
            lhs = AExpr::Bin(Box::new(lhs), op, Box::new(self.parse_mul_div()?));
        }
        Ok(lhs)
    }

    fn parse_mul_div(&mut self) -> Result<AExpr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = match self.chars.get(self.pos) {
                Some('*') => ArithOp::Mul,
                Some('/') => ArithOp::Div,
                Some('%') => ArithOp::Mod,
                _ => break,
            };
            self.pos += 1;
            lhs = AExpr::Bin(Box::new(lhs), op, Box::new(self.parse_unary()?));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<AExpr, String> {
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&'-') {
            self.pos += 1;
            return Ok(AExpr::Neg(Box::new(self.parse_unary()?)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<AExpr, String> {
        self.skip_ws();
        match self.chars.get(self.pos) {
            // `(expr)` is plain grouping; `(expr, ...)` (a leading comma, or
            // an immediate `)`) is a tuple literal -- olive source draws the
            // same distinction.
            Some('(') => {
                self.pos += 1;
                self.skip_ws();
                if self.chars.get(self.pos) == Some(&')') {
                    self.pos += 1;
                    return Ok(AExpr::Tuple(Vec::new()));
                }
                let first = self.parse_add_sub()?;
                self.skip_ws();
                match self.chars.get(self.pos) {
                    Some(')') => {
                        self.pos += 1;
                        Ok(first)
                    }
                    Some(',') => {
                        self.pos += 1;
                        let mut items = vec![first];
                        items.extend(self.parse_comma_list(')')?);
                        Ok(AExpr::Tuple(items))
                    }
                    _ => Err("expected ',' or ')'".to_string()),
                }
            }
            Some('[') => {
                self.pos += 1;
                Ok(AExpr::List(self.parse_comma_list(']')?))
            }
            Some('{') => {
                self.pos += 1;
                Ok(AExpr::Dict(self.parse_dict_body()?))
            }
            Some('"') => Ok(AExpr::Str(self.parse_string_literal()?)),
            Some(c) if c.is_ascii_digit() => self.parse_number(),
            Some(&c) if is_ident_char(c) => {
                if self.eat_word("true") {
                    return Ok(AExpr::Bool(true));
                }
                if self.eat_word("false") {
                    return Ok(AExpr::Bool(false));
                }
                let start = self.pos;
                while matches!(self.chars.get(self.pos), Some(&c) if is_ident_char(c)) {
                    self.pos += 1;
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                self.skip_ws();
                if self.chars.get(self.pos) == Some(&'(') {
                    self.pos += 1;
                    return Ok(AExpr::Construct(name, self.parse_comma_list(')')?));
                }
                self.pos = start;
                Ok(AExpr::Path(scan_and_tokenize_path(
                    &self.chars,
                    &mut self.pos,
                )?))
            }
            _ => Err(format!(
                "unexpected input at position {} in expression",
                self.pos
            )),
        }
    }

    /// Zero or more `parse_add_sub`-level expressions separated by `,`, up
    /// to and including the closing `close` delimiter -- shared by list,
    /// tuple, and constructor-argument literals.
    fn parse_comma_list(&mut self, close: char) -> Result<Vec<AExpr>, String> {
        let mut items = Vec::new();
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&close) {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            items.push(self.parse_add_sub()?);
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some(',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.chars.get(self.pos) == Some(&close) {
                        self.pos += 1;
                        return Ok(items);
                    }
                }
                Some(&c) if c == close => {
                    self.pos += 1;
                    return Ok(items);
                }
                _ => return Err(format!("expected ',' or '{close}'")),
            }
        }
    }

    /// `{k: v, ...}`, up to and including the closing `}`.
    fn parse_dict_body(&mut self) -> Result<Vec<(AExpr, AExpr)>, String> {
        let mut items = Vec::new();
        self.skip_ws();
        if self.chars.get(self.pos) == Some(&'}') {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            let key = self.parse_add_sub()?;
            self.skip_ws();
            if self.chars.get(self.pos) != Some(&':') {
                return Err("expected ':' in dict literal".to_string());
            }
            self.pos += 1;
            let value = self.parse_add_sub()?;
            items.push((key, value));
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some(',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.chars.get(self.pos) == Some(&'}') {
                        self.pos += 1;
                        return Ok(items);
                    }
                }
                Some('}') => {
                    self.pos += 1;
                    return Ok(items);
                }
                _ => return Err("expected ',' or '}' in dict literal".to_string()),
            }
        }
    }

    fn parse_number(&mut self) -> Result<AExpr, String> {
        let start = self.pos;
        while matches!(self.chars.get(self.pos), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.chars.get(self.pos) == Some(&'.')
            && self
                .chars
                .get(self.pos + 1)
                .is_some_and(char::is_ascii_digit)
        {
            is_float = true;
            self.pos += 1;
            while matches!(self.chars.get(self.pos), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            text.parse()
                .map(AExpr::Float)
                .map_err(|_| format!("invalid float literal '{text}'"))
        } else {
            text.parse()
                .map(AExpr::Int)
                .map_err(|_| format!("invalid integer literal '{text}'"))
        }
    }

    pub(crate) fn parse_string_literal(&mut self) -> Result<String, String> {
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
}

/// Root + descent for a path, resolved against a captured frame's own
/// cells -- shared by `evaluate`'s arithmetic path, `conditions.rs`'s
/// pre-stop operands, and anything else that has `(fn_id, cells)` on hand
/// rather than a live `VarStore` reference.
pub(crate) fn resolve_path_value(
    shared: &EngineShared,
    fn_id: u32,
    cells: &[i64],
    tokens: &[Token],
) -> Result<(i64, Type), String> {
    let Some(Token::Ident(root)) = tokens.first() else {
        return Err("empty path".to_string());
    };
    let fn_cells = shared.fn_cells(fn_id);
    let idx = fn_cells
        .iter()
        .position(|c| c.name == *root)
        .ok_or_else(|| format!("no such variable: {root}"))?;
    let mut raw = cells.get(idx).copied().unwrap_or(0);
    let mut ty = fn_cells[idx].ty.clone();

    for tok in &tokens[1..] {
        let key = token_key(tok);
        let (_, cval, cty) = values::children_raw(shared, raw, &ty)
            .into_iter()
            .find(|(n, _, _)| *n == key)
            .ok_or_else(|| format!("no such member: {key}"))?;
        raw = cval;
        ty = cty;
    }
    Ok((raw, ty))
}

/// Decodes a raw cell word into a comparable/arithmetic-ready `Value`.
/// Tag-encoded scalar unions decode via the runtime; everything else reads
/// straight off its concrete type. Structs, collections, and enums have no
/// `Value` form -- they're neither comparable nor arithmetic operands.
pub(crate) fn decode_value(shared: &EngineShared, raw: i64, ty: &Type) -> Result<Value, String> {
    if ty.is_tag_encoded_union() {
        let Some((kind, payload)) = values::any_decode(shared, raw) else {
            return Err("runtime decoder unavailable".to_string());
        };
        return match kind {
            0 => Err("value is None".to_string()),
            2 => Ok(Value::Bool(payload != 0)),
            3 => Ok(Value::Float(f64::from_bits(payload as u64))),
            4 => Ok(Value::Str(values::str_value(shared, payload))),
            5 => Err("cannot use a struct or collection union member in an expression".to_string()),
            _ => Ok(Value::Int(payload)),
        };
    }
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
        other => Err(format!("cannot use a {other} value in an expression")),
    }
}

/// Evaluates a parsed arithmetic expression against a captured `(fn_id,
/// cells)` frame -- the same pair `conditions.rs`'s hook-time operands and
/// `evaluate`'s live-frame lookup both already have on hand.
pub(crate) fn eval_arith(
    shared: &EngineShared,
    fn_id: u32,
    cells: &[i64],
    expr: &AExpr,
) -> Result<Value, String> {
    match expr {
        AExpr::Int(n) => Ok(Value::Int(*n)),
        AExpr::Float(f) => Ok(Value::Float(*f)),
        AExpr::Bool(b) => Ok(Value::Bool(*b)),
        AExpr::Str(s) => Ok(Value::Str(s.clone())),
        AExpr::Path(tokens) => {
            let (raw, ty) = resolve_path_value(shared, fn_id, cells, tokens)?;
            decode_value(shared, raw, &ty)
        }
        AExpr::Neg(inner) => match eval_arith(shared, fn_id, cells, inner)? {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(format!("cannot negate {other}")),
        },
        AExpr::Bin(l, op, r) => {
            let lv = eval_arith(shared, fn_id, cells, l)?;
            let rv = eval_arith(shared, fn_id, cells, r)?;
            apply_arith(*op, lv, rv)
        }
        AExpr::List(_) | AExpr::Tuple(_) | AExpr::Dict(_) | AExpr::Construct(_, _) => {
            Err("a list/tuple/dict/struct construction is not a plain value here".to_string())
        }
    }
}

fn apply_arith(op: ArithOp, a: Value, b: Value) -> Result<Value, String> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => match op {
            ArithOp::Add => Ok(Value::Str(x + &y)),
            _ => Err(format!(
                "cannot apply that operator to strings '{x}' and '{y}'"
            )),
        },
        (Value::Int(x), Value::Int(y)) => match op {
            ArithOp::Add => Ok(Value::Int(x.wrapping_add(y))),
            ArithOp::Sub => Ok(Value::Int(x.wrapping_sub(y))),
            ArithOp::Mul => Ok(Value::Int(x.wrapping_mul(y))),
            ArithOp::Div => x
                .checked_div(y)
                .map(Value::Int)
                .ok_or_else(|| "division by zero".to_string()),
            ArithOp::Mod => x
                .checked_rem(y)
                .map(Value::Int)
                .ok_or_else(|| "division by zero".to_string()),
        },
        (a, b) => {
            let (x, y) = (as_f64(a)?, as_f64(b)?);
            match op {
                ArithOp::Add => Ok(Value::Float(x + y)),
                ArithOp::Sub => Ok(Value::Float(x - y)),
                ArithOp::Mul => Ok(Value::Float(x * y)),
                ArithOp::Div => Ok(Value::Float(x / y)),
                ArithOp::Mod => Ok(Value::Float(x % y)),
            }
        }
    }
}

fn as_f64(v: Value) -> Result<f64, String> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        other => Err(format!("cannot use {other} in arithmetic")),
    }
}
