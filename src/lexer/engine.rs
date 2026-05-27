use std::collections::VecDeque;

use super::error::{LexError, LexResult};
use super::token::{Comment, CommentKind, Token, TokenKind};

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    indent_stack: Vec<usize>,
    pending: VecDeque<Token>,
    delimiter_stack: Vec<(char, usize, usize)>,
    file_id: usize,
    comments: Vec<Comment>,
}

impl Lexer {
    pub fn new(source: &str, file_id: usize) -> Self {
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            indent_stack: vec![0],
            pending: VecDeque::new(),
            delimiter_stack: Vec::new(),
            file_id,
            comments: Vec::new(),
        }
    }

    /// Comments recovered while lexing, in source order. Populated as a side effect
    /// of skipping; the token stream itself never contains them.
    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    fn record_comment(&mut self, start: usize, kind: CommentKind) {
        let text: String = self.source[start..self.pos].iter().collect();
        self.comments.push(Comment {
            kind,
            text,
            span: (start, self.pos),
        });
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn err(&self, msg: impl Into<String>) -> LexError {
        LexError {
            message: msg.into(),
            line: self.line,
            col: self.col,
            start: self.pos,
            end: self.pos + 1,
        }
    }

    fn make_tok(
        &self,
        kind: TokenKind,
        value: impl Into<String>,
        line: usize,
        col: usize,
        start: usize,
    ) -> Token {
        Token {
            kind,
            value: value.into(),
            line,
            col,
            span: (start, self.pos),
            file_id: self.file_id,
        }
    }

    fn skip_inline_whitespace(&mut self) {
        loop {
            let mut skipped = false;
            while matches!(self.peek(), Some(' ') | Some('\t') | Some('\r')) {
                self.advance();
                skipped = true;
            }
            if self.peek() == Some('/') && self.peek_next() == Some('/') {
                let cstart = self.pos;
                while matches!(self.peek(), Some(c) if c != '\n') {
                    self.advance();
                }
                self.record_comment(cstart, CommentKind::Line);
                skipped = true;
            }
            if self.peek() == Some('/') && self.peek_next() == Some('*') {
                let cstart = self.pos;
                self.advance();
                self.advance();
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek_next() == Some('/') {
                        self.advance();
                        self.advance();
                        break;
                    }
                    self.advance();
                }
                self.record_comment(cstart, CommentKind::Block);
                skipped = true;
            }
            if !skipped {
                break;
            }
        }
    }

    fn handle_indent(&mut self, nl_line: usize, nl_pos: usize) -> LexResult<Option<Token>> {
        let mut depth = 0usize;
        let mut has_tab = false;
        let mut has_space = false;
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            match self.advance().unwrap() {
                '\t' => {
                    has_tab = true;
                    depth += 4;
                }
                _ => {
                    has_space = true;
                    depth += 1;
                }
            }
        }
        if has_tab && has_space {
            return Err(LexError {
                message: "mixed tabs and spaces in indentation".into(),
                line: nl_line,
                col: 1,
                start: nl_pos,
                end: self.pos,
            });
        }

        if self.peek() == Some('/') && self.peek_next() == Some('/') {
            let cstart = self.pos;
            while matches!(self.peek(), Some(c) if c != '\n') {
                self.advance();
            }
            self.record_comment(cstart, CommentKind::Line);
            return Ok(None);
        }
        if self.peek() == Some('/') && self.peek_next() == Some('*') {
            let cstart = self.pos;
            self.advance();
            self.advance();
            while let Some(c) = self.peek() {
                if c == '*' && self.peek_next() == Some('/') {
                    self.advance();
                    self.advance();
                    break;
                }
                self.advance();
            }
            self.record_comment(cstart, CommentKind::Block);
            return Ok(None);
        }
        if matches!(self.peek(), Some('\n') | None) {
            while matches!(self.peek(), Some(c) if c != '\n') {
                self.advance();
            }
            return Ok(None);
        }

        let current = *self.indent_stack.last().unwrap();

        if depth > current {
            self.indent_stack.push(depth);
            self.pending.push_back(Token {
                kind: TokenKind::Indent,
                value: String::new(),
                line: nl_line,
                col: 1,
                span: (nl_pos, self.pos),
                file_id: self.file_id,
            });
        } else if depth < current {
            while *self.indent_stack.last().unwrap() > depth {
                self.indent_stack.pop();
                self.pending.push_back(Token {
                    kind: TokenKind::Dedent,
                    value: String::new(),
                    line: nl_line,
                    col: 1,
                    span: (nl_pos, self.pos),
                    file_id: self.file_id,
                });
            }
            if *self.indent_stack.last().unwrap() != depth {
                return Err(LexError {
                    message: format!(
                        "dedent to level {} does not match any outer indentation block",
                        depth
                    ),
                    line: nl_line,
                    col: 1,
                    start: nl_pos,
                    end: self.pos,
                });
            }
        }

        Ok(Some(Token {
            kind: TokenKind::Newline,
            value: "\n".into(),
            line: nl_line,
            col: 1,
            span: (nl_pos, nl_pos + 1),
            file_id: self.file_id,
        }))
    }

    fn read_string(
        &mut self,
        quote: char,
        line: usize,
        col: usize,
        start: usize,
    ) -> LexResult<Token> {
        let triple = self.peek() == Some(quote) && self.peek_next() == Some(quote);
        if triple {
            self.advance();
            self.advance();
        }

        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(LexError {
                        message: "unterminated string literal".into(),
                        line,
                        col,
                        start,
                        end: self.pos,
                    });
                }
                Some(c) if !triple && c == '\n' => {
                    return Err(LexError {
                        message:
                            "newline inside single-quoted string; use triple quotes for multiline"
                                .into(),
                        line,
                        col,
                        start,
                        end: self.pos,
                    });
                }
                Some(c) if c == quote => {
                    if triple {
                        if self.peek() == Some(quote) && self.peek_next() == Some(quote) {
                            self.advance();
                            self.advance();
                            break;
                        }
                        s.push(c);
                    } else {
                        break;
                    }
                }
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('0') => s.push('\0'),
                    Some('\\') => s.push('\\'),
                    Some('\'') => s.push('\''),
                    Some('"') => s.push('"'),
                    Some('\n') => {}
                    Some('x') => {
                        let hi = self.advance();
                        let lo = self.advance();
                        match (
                            hi.and_then(|c| c.to_digit(16)),
                            lo.and_then(|c| c.to_digit(16)),
                        ) {
                            (Some(h), Some(l)) => s.push(((h * 16 + l) as u8) as char),
                            _ => {
                                return Err(LexError {
                                    message: "invalid `\\x` escape: expected two hex digits".into(),
                                    line,
                                    col,
                                    start: self.pos - 1,
                                    end: self.pos,
                                });
                            }
                        }
                    }
                    Some('u') => {
                        let mut hex = String::new();
                        if self.peek() == Some('{') {
                            self.advance();
                            while let Some(c) = self.peek() {
                                if c == '}' {
                                    self.advance();
                                    break;
                                }
                                hex.push(c);
                                self.advance();
                            }
                        }
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) => s.push(ch),
                            None => {
                                return Err(LexError {
                                    message: "invalid `\\u{...}` escape: expected a Unicode scalar"
                                        .into(),
                                    line,
                                    col,
                                    start: self.pos - 1,
                                    end: self.pos,
                                });
                            }
                        }
                    }
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                    None => {
                        return Err(LexError {
                            message: "unterminated escape sequence".into(),
                            line,
                            col,
                            start: self.pos - 1,
                            end: self.pos,
                        });
                    }
                },
                Some(c) => s.push(c),
            }
        }
        Ok(self.make_tok(TokenKind::String, s, line, col, start))
    }

    fn read_number(
        &mut self,
        first: char,
        line: usize,
        col: usize,
        start: usize,
    ) -> LexResult<Token> {
        if first == '0' {
            match self.peek() {
                Some('x') | Some('X') => {
                    self.advance();
                    let mut num = String::from("0x");
                    if !matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                        return Err(LexError {
                            message: "invalid hexadecimal literal".into(),
                            line,
                            col,
                            start,
                            end: self.pos,
                        });
                    }
                    while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                        num.push(self.advance().unwrap());
                    }
                    return Ok(self.make_tok(TokenKind::Integer, num, line, col, start));
                }
                Some('o') | Some('O') => {
                    self.advance();
                    let mut num = String::from("0o");
                    if !matches!(self.peek(), Some(c) if matches!(c, '0'..='7')) {
                        return Err(LexError {
                            message: "invalid octal literal".into(),
                            line,
                            col,
                            start,
                            end: self.pos,
                        });
                    }
                    while matches!(self.peek(), Some(c) if matches!(c, '0'..='7')) {
                        num.push(self.advance().unwrap());
                    }
                    return Ok(self.make_tok(TokenKind::Integer, num, line, col, start));
                }
                Some('b') | Some('B') => {
                    self.advance();
                    let mut num = String::from("0b");
                    if !matches!(self.peek(), Some('0' | '1')) {
                        return Err(LexError {
                            message: "invalid binary literal".into(),
                            line,
                            col,
                            start,
                            end: self.pos,
                        });
                    }
                    while matches!(self.peek(), Some('0' | '1')) {
                        num.push(self.advance().unwrap());
                    }
                    return Ok(self.make_tok(TokenKind::Integer, num, line, col, start));
                }
                _ => {}
            }
        }

        let mut num = String::from(first);
        let mut is_float = false;

        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            num.push(self.advance().unwrap());
        }

        if self.peek() == Some('.') && matches!(self.peek_next(), Some(c) if c.is_ascii_digit()) {
            is_float = true;
            num.push(self.advance().unwrap());
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                num.push(self.advance().unwrap());
            }
        }

        if matches!(self.peek(), Some('e') | Some('E')) {
            is_float = true;
            num.push(self.advance().unwrap());
            if matches!(self.peek(), Some('+') | Some('-')) {
                num.push(self.advance().unwrap());
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(LexError {
                    message: format!("invalid scientific notation: '{}'", num),
                    line,
                    col,
                    start,
                    end: self.pos,
                });
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                num.push(self.advance().unwrap());
            }
        }

        Ok(self.make_tok(
            if is_float {
                TokenKind::Float
            } else {
                TokenKind::Integer
            },
            num,
            line,
            col,
            start,
        ))
    }

    fn read_ident(
        &mut self,
        first: char,
        line: usize,
        col: usize,
        start: usize,
    ) -> LexResult<Token> {
        let mut ident = String::from(first);

        if first == 'f' && (self.peek() == Some('"') || self.peek() == Some('\'')) {
            let quote = self.advance().unwrap();
            let mut tok = self.read_string(quote, line, col, start)?;
            tok.kind = TokenKind::FString;
            return Ok(tok);
        }

        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            ident.push(self.advance().unwrap());
        }
        let kind = match ident.as_str() {
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "const" => TokenKind::Const,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "elif" => TokenKind::Elif,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "True" => TokenKind::True,
            "False" => TokenKind::False,
            "not" => TokenKind::Not,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "pass" => TokenKind::Pass,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "try" => TokenKind::Try,
            "as" => TokenKind::As,
            "assert" => TokenKind::Assert,
            "import" => TokenKind::Import,
            "from" => TokenKind::From,
            "struct" => TokenKind::Struct,
            "impl" => TokenKind::Impl,
            "trait" => TokenKind::Trait,
            "mut" => TokenKind::Mut,
            "enum" => TokenKind::Enum,
            "match" => TokenKind::Match,
            "async" => TokenKind::Async,
            "await" => TokenKind::Await,
            "case" => TokenKind::Case,
            "unsafe" => TokenKind::Unsafe,
            "defer" => TokenKind::Defer,
            "None" => TokenKind::Null,
            "with" => TokenKind::With,
            "lambda" => TokenKind::Lambda,

            "_" => TokenKind::Underscore,
            _ => TokenKind::Identifier,
        };
        Ok(self.make_tok(kind, ident, line, col, start))
    }

    pub fn next_token(&mut self) -> LexResult<Token> {
        loop {
            if let Some(tok) = self.pending.pop_front() {
                return Ok(tok);
            }

            self.skip_inline_whitespace();

            let line = self.line;
            let col = self.col;
            let start = self.pos;

            let ch = match self.advance() {
                None => {
                    if let Some(&(open, dl, dc)) = self.delimiter_stack.last() {
                        return Err(LexError {
                            message: format!("unclosed '{}' opened at {}:{}", open, dl, dc),
                            line,
                            col,
                            start: self.pos,
                            end: self.pos + 1,
                        });
                    }

                    if self.indent_stack.len() > 1 {
                        let n = self.indent_stack.len() - 1;
                        self.indent_stack.truncate(1);
                        for _ in 0..n {
                            self.pending.push_back(Token {
                                kind: TokenKind::Dedent,
                                value: String::new(),
                                line,
                                col,
                                span: (start, start),
                                file_id: self.file_id,
                            });
                        }
                        self.pending.push_back(Token {
                            kind: TokenKind::Eof,
                            value: String::new(),
                            line,
                            col,
                            span: (start, start),
                            file_id: self.file_id,
                        });
                        return Ok(Token {
                            kind: TokenKind::Newline,
                            value: "\n".into(),
                            line,
                            col,
                            span: (start, start),
                            file_id: self.file_id,
                        });
                    }
                    return Ok(Token {
                        kind: TokenKind::Eof,
                        value: String::new(),
                        line,
                        col,
                        span: (start, start),
                        file_id: self.file_id,
                    });
                }
                Some(c) => c,
            };

            let tok = match ch {
                '\n' => {
                    if !self.delimiter_stack.is_empty() {
                        continue;
                    }
                    match self.handle_indent(line, start)? {
                        Some(tok) => tok,
                        None => continue,
                    }
                }

                '\\' => {
                    if self.peek() == Some('\n') {
                        self.advance();
                        continue;
                    }
                    return Err(self.err("unexpected '\\'"));
                }

                '.' if matches!(self.peek(), Some(c) if c.is_ascii_digit()) => {
                    let mut num = String::from("0.");
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        num.push(self.advance().unwrap());
                    }
                    if matches!(self.peek(), Some('e') | Some('E')) {
                        num.push(self.advance().unwrap());
                        if matches!(self.peek(), Some('+') | Some('-')) {
                            num.push(self.advance().unwrap());
                        }
                        if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                            return Err(self.err("invalid scientific notation"));
                        }
                        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                            num.push(self.advance().unwrap());
                        }
                    }
                    self.make_tok(TokenKind::Float, num, line, col, start)
                }

                c if c.is_ascii_digit() => self.read_number(c, line, col, start)?,

                '"' | '\'' => self.read_string(ch, line, col, start)?,

                c if c.is_alphabetic() || c == '_' => self.read_ident(c, line, col, start)?,

                '=' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::DoubleEqual, "==", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Equal, "=", line, col, start)
                    }
                }
                '!' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::NotEqual, "!=", line, col, start)
                    } else {
                        return Err(self.err("unexpected '!': use 'not' for logical negation"));
                    }
                }
                '<' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::LessEqual, "<=", line, col, start)
                    } else if self.peek() == Some('<') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            self.make_tok(TokenKind::ShlEqual, "<<=", line, col, start)
                        } else {
                            self.make_tok(TokenKind::Shl, "<<", line, col, start)
                        }
                    } else {
                        self.make_tok(TokenKind::Less, "<", line, col, start)
                    }
                }
                '>' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::GreaterEqual, ">=", line, col, start)
                    } else if self.peek() == Some('>') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            self.make_tok(TokenKind::ShrEqual, ">>=", line, col, start)
                        } else {
                            self.make_tok(TokenKind::Shr, ">>", line, col, start)
                        }
                    } else {
                        self.make_tok(TokenKind::Greater, ">", line, col, start)
                    }
                }
                ':' => {
                    if self.peek() == Some(':') {
                        self.advance();
                        self.make_tok(TokenKind::DoubleColon, "::", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Colon, ":", line, col, start)
                    }
                }
                '+' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::PlusEqual, "+=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Plus, "+", line, col, start)
                    }
                }
                '-' => {
                    if self.peek() == Some('>') {
                        self.advance();
                        self.make_tok(TokenKind::Arrow, "->", line, col, start)
                    } else if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::MinusEqual, "-=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Minus, "-", line, col, start)
                    }
                }
                '*' => {
                    if self.peek() == Some('*') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            self.make_tok(TokenKind::DoubleStarEqual, "**=", line, col, start)
                        } else {
                            self.make_tok(TokenKind::DoubleStar, "**", line, col, start)
                        }
                    } else if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::StarEqual, "*=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Star, "*", line, col, start)
                    }
                }
                '/' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::SlashEqual, "/=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Slash, "/", line, col, start)
                    }
                }
                '%' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::PercentEqual, "%=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Percent, "%", line, col, start)
                    }
                }

                '(' => {
                    self.delimiter_stack.push(('(', line, col));
                    self.make_tok(TokenKind::LParen, "(", line, col, start)
                }
                ')' => {
                    match self.delimiter_stack.pop() {
                        None => return Err(self.err("unmatched ')'")),
                        Some(('(', _, _)) => {}
                        Some((open, dl, dc)) => {
                            return Err(LexError {
                                message: format!(
                                    "mismatched delimiter: '{}' opened at {}:{} closed by ')'",
                                    open, dl, dc
                                ),
                                line,
                                col,
                                start: self.pos - 1,
                                end: self.pos,
                            });
                        }
                    }
                    self.make_tok(TokenKind::RParen, ")", line, col, start)
                }
                '[' => {
                    self.delimiter_stack.push(('[', line, col));
                    self.make_tok(TokenKind::LBracket, "[", line, col, start)
                }
                ']' => {
                    match self.delimiter_stack.pop() {
                        None => return Err(self.err("unmatched ']'")),
                        Some(('[', _, _)) => {}
                        Some((open, dl, dc)) => {
                            return Err(LexError {
                                message: format!(
                                    "mismatched delimiter: '{}' opened at {}:{} closed by ']'",
                                    open, dl, dc
                                ),
                                line,
                                col,
                                start: self.pos - 1,
                                end: self.pos,
                            });
                        }
                    }
                    self.make_tok(TokenKind::RBracket, "]", line, col, start)
                }
                '{' => {
                    self.delimiter_stack.push(('{', line, col));
                    self.make_tok(TokenKind::LBrace, "{", line, col, start)
                }
                '}' => {
                    match self.delimiter_stack.pop() {
                        None => return Err(self.err("unmatched '}'")),
                        Some(('{', _, _)) => {}
                        Some((open, dl, dc)) => {
                            return Err(LexError {
                                message: format!(
                                    "mismatched delimiter: '{}' opened at {}:{} closed by '}}'",
                                    open, dl, dc
                                ),
                                line,
                                col,
                                start: self.pos - 1,
                                end: self.pos,
                            });
                        }
                    }
                    self.make_tok(TokenKind::RBrace, "}", line, col, start)
                }

                ',' => self.make_tok(TokenKind::Comma, ",", line, col, start),
                '.' if self.peek() == Some('.') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::DotDotEq, "..=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::DotDot, "..", line, col, start)
                    }
                }
                '.' => self.make_tok(TokenKind::Dot, ".", line, col, start),
                ';' => self.make_tok(TokenKind::Semicolon, ";", line, col, start),
                '&' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::AmpersandEqual, "&=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Ampersand, "&", line, col, start)
                    }
                }
                '@' => self.make_tok(TokenKind::At, "@", line, col, start),
                '|' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::PipeEqual, "|=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Pipe, "|", line, col, start)
                    }
                }
                '^' => {
                    if self.peek() == Some('=') {
                        self.advance();
                        self.make_tok(TokenKind::CaretEqual, "^=", line, col, start)
                    } else {
                        self.make_tok(TokenKind::Caret, "^", line, col, start)
                    }
                }
                '~' => self.make_tok(TokenKind::Tilde, "~", line, col, start),
                '?' => self.make_tok(TokenKind::Question, "?", line, col, start),
                '#' => self.make_tok(TokenKind::Hash, "#", line, col, start),

                other => return Err(self.err(format!("unexpected character {:?}", other))),
            };

            return Ok(tok);
        }
    }

    pub fn tokenise(&mut self) -> LexResult<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_and_advance() {
        let mut lex = Lexer::new("ab", 0);
        assert_eq!(lex.peek(), Some('a'));
        assert_eq!(lex.advance(), Some('a'));
        assert_eq!(lex.peek(), Some('b'));
        assert_eq!(lex.pos, 1);
        assert_eq!(lex.col, 2);
    }

    #[test]
    fn peek_returns_none_at_end() {
        let mut lex = Lexer::new("a", 0);
        lex.advance();
        assert_eq!(lex.peek(), None);
    }

    #[test]
    fn advance_tracks_newlines() {
        let mut lex = Lexer::new("a\nb", 0);
        assert_eq!(lex.advance(), Some('a'));
        assert_eq!(lex.advance(), Some('\n'));
        assert_eq!(lex.line, 2);
        assert_eq!(lex.col, 1);
        assert_eq!(lex.advance(), Some('b'));
        assert_eq!(lex.col, 2);
    }

    #[test]
    fn err_uses_current_position() {
        let mut lex = Lexer::new("abc\ndef", 0);
        lex.advance(); // 'a', col=2
        let e = lex.err("error msg");
        assert_eq!(e.line, 1);
        assert_eq!(e.col, 2);
        assert_eq!(e.message, "error msg");
    }

    #[test]
    fn make_tok_creates_token() {
        let lex = Lexer::new("hello", 0);
        let tok = lex.make_tok(TokenKind::Identifier, "hello", 1, 1, 0);
        assert_eq!(tok.kind, TokenKind::Identifier);
        assert_eq!(tok.value, "hello");
        assert_eq!(tok.file_id, 0);
    }

    #[test]
    fn skip_inline_whitespace_skips_spaces() {
        let mut lex = Lexer::new("   x", 0);
        lex.skip_inline_whitespace();
        assert_eq!(lex.peek(), Some('x'));
    }

    #[test]
    fn skip_inline_whitespace_skips_line_comment() {
        let mut lex = Lexer::new("// comment\n", 0);
        lex.skip_inline_whitespace();
        assert_eq!(lex.peek(), Some('\n'));
    }

    #[test]
    fn skip_inline_whitespace_skips_block_comment() {
        let mut lex = Lexer::new("/* a */\n", 0);
        lex.skip_inline_whitespace();
        assert_eq!(lex.peek(), Some('\n'));
    }

    #[test]
    fn read_number_handles_hex() {
        let mut lex = Lexer::new("0xFF", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Integer);
        assert_eq!(tok.value, "0xFF");
    }

    #[test]
    fn read_number_handles_octal() {
        let mut lex = Lexer::new("0o77", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Integer);
        assert_eq!(tok.value, "0o77");
    }

    #[test]
    fn read_number_handles_binary() {
        let mut lex = Lexer::new("0b1010", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Integer);
        assert_eq!(tok.value, "0b1010");
    }

    #[test]
    fn read_number_handles_float() {
        let mut lex = Lexer::new("3.14", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Float);
        assert_eq!(tok.value, "3.14");
    }

    #[test]
    fn read_number_handles_scientific() {
        let mut lex = Lexer::new("1.5e10", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Float);
        assert_eq!(tok.value, "1.5e10");
    }

    #[test]
    fn read_number_handles_integer() {
        let mut lex = Lexer::new("42", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_number(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Integer);
        assert_eq!(tok.value, "42");
    }

    #[test]
    fn read_string_basic() {
        let mut lex = Lexer::new("\"hello\"", 0);
        lex.advance(); // skip opening quote
        let tok = lex.read_string('"', 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::String);
        assert_eq!(tok.value, "hello");
    }

    #[test]
    fn read_string_with_escapes() {
        let mut lex = Lexer::new("\"hello\\nworld\"", 0);
        lex.advance(); // skip opening quote
        let tok = lex.read_string('"', 1, 1, 0).unwrap();
        assert_eq!(tok.value, "hello\nworld");
    }

    #[test]
    fn read_string_triple_quoted() {
        let mut lex = Lexer::new("\"\"\"hello\"\"\"", 0);
        lex.advance(); // skip first opening quote
        let tok = lex.read_string('"', 1, 1, 0).unwrap();
        assert_eq!(tok.value, "hello");
    }

    #[test]
    fn read_ident_distinguishes_keyword() {
        let mut lex = Lexer::new("fn", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_ident(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Fn);
    }

    #[test]
    fn read_ident_plain_identifier() {
        let mut lex = Lexer::new("myVar", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_ident(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Identifier);
        assert_eq!(tok.value, "myVar");
    }

    #[test]
    fn read_ident_underscore() {
        let mut lex = Lexer::new("_", 0);
        let ch = lex.advance().unwrap();
        let tok = lex.read_ident(ch, 1, 1, 0).unwrap();
        assert_eq!(tok.kind, TokenKind::Underscore);
        assert_eq!(tok.value, "_");
    }

    #[test]
    fn handle_indent_creates_indent_token() {
        let mut lex = Lexer::new("\n    pass", 0);
        lex.advance(); // consume \n, now at first space
        let result = lex.handle_indent(1, 0).unwrap();
        assert_eq!(
            result.as_ref().map(|t| t.kind.clone()),
            Some(TokenKind::Newline)
        );
        assert_eq!(lex.pending.len(), 1);
        assert_eq!(lex.pending[0].kind, TokenKind::Indent);
    }
}
