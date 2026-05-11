use super::comments::{CommentWeaver, comment_text};
use super::doc::*;
use super::syntax::{augop_str, expr_prec, is_decl};
use crate::lexer::Comment;
use crate::parser::ast::*;

/// Turns an AST plus the recovered comments into a single layout `Doc`. Holds the
/// original source (as chars, matching the lexer's offset space) so literals and a
/// handful of surface-ambiguous constructs can be reproduced verbatim from their spans.
pub struct Lowerer<'a> {
    src: &'a [char],
    pub cw: CommentWeaver,
}

pub(super) struct Item {
    start: usize,
    end: usize,
    is_decl: bool,
    doc: Doc,
}

impl Item {
    pub(super) fn new(start: usize, end: usize, is_decl: bool, doc: Doc) -> Self {
        Self {
            start,
            end,
            is_decl,
            doc,
        }
    }
}

impl<'a> Lowerer<'a> {
    pub fn new(src: &'a [char], comments: &[Comment]) -> Self {
        Self {
            src,
            cw: CommentWeaver::new(src, comments),
        }
    }

    pub fn program(&mut self, prog: &Program) -> Doc {
        self.weave_stmts(&prog.stmts, usize::MAX, true)
    }

    pub(super) fn slice(&self, start: usize, end: usize) -> String {
        self.src[start..end].iter().collect()
    }

    fn verbatim(&self, span: crate::span::Span) -> Doc {
        text(self.slice(span.start, span.end))
    }

    /// First non-whitespace char of a span, used to recover whether an ambiguous
    /// construct (a tuple) was written with surrounding parentheses.
    fn span_starts_with(&self, span: crate::span::Span, c: char) -> bool {
        self.src[span.start..span.end]
            .iter()
            .find(|ch| !ch.is_whitespace())
            .copied()
            == Some(c)
    }

    pub(super) fn comment_doc(&self, c: &Comment) -> Doc {
        let txt = comment_text(c);
        if txt.contains('\n') {
            join(hardline(), txt.lines().map(text))
        } else {
            text(txt)
        }
    }

    /// Join already-lowered items with hardlines, preserving a single blank line where
    /// the source had one or more, and forcing a blank between adjacent top-level
    /// declarations.
    pub(super) fn assemble(items: Vec<Item>) -> Doc {
        let mut out = Doc::Nil;
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                out = concat(out, hardline());
                let prev = &items[i - 1];
                let blank = it.start > prev.end + 1 || (prev.is_decl && it.is_decl);
                if blank {
                    out = concat(out, hardline());
                }
            }
            out = concat(out, it.doc.clone());
        }
        out
    }

    /// Lower a statement sequence, weaving in standalone and trailing comments by span.
    /// `bound` is the exclusive char offset past which comments belong to an outer
    /// scope; everything before it that is still unconsumed is flushed here.
    pub(super) fn weave_stmts(&mut self, stmts: &[Stmt], bound: usize, top: bool) -> Doc {
        let col = stmts
            .first()
            .map(|s| self.cw.column_of(s.span.start))
            .unwrap_or(0);
        let mut items: Vec<Item> = Vec::new();
        for (i, s) in stmts.iter().enumerate() {
            self.take_leading(&mut items, s.span.start);
            let sl = self.cw.line_of(s.span.start);
            // A statement owns the comments up to where its next sibling begins, so
            // trailing comments inside its block aren't stranded at the outer indent.
            let region_end = stmts.get(i + 1).map(|n| n.span.start).unwrap_or(bound);
            let mut doc = self.stmt(s, region_end);
            let el = self.cw.line_of(s.span.end.saturating_sub(1));
            if let Some(c) = self.cw.take_trailing(el, s.span.end) {
                doc = concat(doc, concat(text("  "), self.comment_doc(&c)));
            }
            items.push(Item {
                start: sl,
                end: el,
                is_decl: top && is_decl(&s.kind),
                doc,
            });
        }
        self.flush_block(&mut items, bound, col);
        Self::assemble(items)
    }

    /// Absorb a block's trailing comments, those indented at least `col`. Shallower
    /// comments belong to an enclosing scope and are left for it to place.
    pub(super) fn flush_block(&mut self, items: &mut Vec<Item>, bound: usize, col: usize) {
        while let Some(c) = self.cw.take_before_col(bound, col) {
            let l = self.cw.line_of(c.span.0);
            let doc = self.comment_doc(&c);
            items.push(Item::new(
                l,
                self.cw.line_of(c.span.1.saturating_sub(1)),
                false,
                doc,
            ));
        }
    }

    pub(super) fn take_leading(&mut self, items: &mut Vec<Item>, before: usize) {
        while let Some(c) = self.cw.take_before(before) {
            let l = self.cw.line_of(c.span.0);
            let doc = self.comment_doc(&c);
            items.push(Item {
                start: l,
                end: self.cw.line_of(c.span.1.saturating_sub(1)),
                is_decl: false,
                doc,
            });
        }
    }

    /// `header:` followed by an indented, comment-woven body.
    pub(super) fn suite(&mut self, header: Doc, body: &[Stmt], bound: usize) -> Doc {
        let inner = self.weave_stmts(body, bound, false);
        concat(header, nest(INDENT, concat(hardline(), inner)))
    }

    /// Comment bound for a loop body: it ends where its `else` clause begins.
    fn else_bound(&self, else_body: &Option<Vec<Stmt>>, region_end: usize) -> usize {
        else_body
            .as_ref()
            .and_then(|b| b.first())
            .map(|s| s.span.start)
            .unwrap_or(region_end)
    }

    /// Append a `while`/`for` else clause, if present.
    fn with_else(&mut self, out: Doc, else_body: &Option<Vec<Stmt>>, region_end: usize) -> Doc {
        match else_body {
            Some(eb) => {
                let e = self.suite(text("else:"), eb, region_end);
                concat(out, concat(hardline(), e))
            }
            None => out,
        }
    }

    /// Suite used by expression-position blocks (match arms, `async:`), which have no
    /// enclosing statement span to bound comments; the body's own extent serves.
    pub(super) fn suite_expr(&mut self, header: Doc, body: &[Stmt]) -> Doc {
        let bound = body.last().map(|s| s.span.end).unwrap_or(0);
        self.suite(header, body, bound)
    }

    /// True when the span's leading text is the keyword `kw` followed by whitespace.
    /// Distinguishes prefix `try x` / `await x` from postfix `x?` / `x.await`.
    fn span_kw_prefix(&self, span: crate::span::Span, kw: &str) -> bool {
        let end = span.start + kw.len();
        if end >= span.end {
            return false;
        }
        kw.chars()
            .enumerate()
            .all(|(i, c)| self.src[span.start + i] == c)
            && self.src[end].is_whitespace()
    }

    pub(super) fn span_starts_with_try(&self, span: crate::span::Span) -> bool {
        self.span_kw_prefix(span, "try")
    }

    pub(super) fn span_starts_with_await(&self, span: crate::span::Span) -> bool {
        self.span_kw_prefix(span, "await")
    }

    pub(super) fn stmt(&mut self, s: &Stmt, region_end: usize) -> Doc {
        let bound = region_end;
        match &s.kind {
            StmtKind::Fn { .. } => self.lower_fn(s, region_end),
            StmtKind::Struct { .. } => self.lower_struct(s, region_end),
            StmtKind::Enum { .. } => self.lower_enum(s, region_end),
            StmtKind::Impl {
                type_params,
                trait_name,
                type_name,
                body,
            } => {
                let mut head = concat(text("impl"), self.type_params(type_params));
                head = concat(head, text(" "));
                if let Some(tr) = trait_name {
                    head = concat_all([head, text(tr.to_string()), text(" for ")]);
                }
                head = concat(head, text(type_name.to_string()));
                self.suite(concat(head, text(":")), body, bound)
            }
            StmtKind::Trait {
                name,
                type_params,
                methods,
            } => {
                let head = concat_all([
                    text("trait "),
                    text(name.clone()),
                    self.type_params(type_params),
                    text(":"),
                ]);
                self.suite(head, methods, bound)
            }
            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                // Each clause owns comments only up to where the next clause begins.
                let else_start = else_body
                    .as_ref()
                    .and_then(|b| b.first())
                    .map(|s| s.span.start);
                let clause_start = |i: usize| {
                    elif_clauses
                        .get(i)
                        .map(|(c, _)| c.span.start)
                        .or(else_start)
                        .unwrap_or(region_end)
                };
                let head = concat_all([text("if "), self.expr(condition), text(":")]);
                let mut out = self.suite(head, then_body, clause_start(0));
                for (i, (cond, body)) in elif_clauses.iter().enumerate() {
                    let h = concat_all([text("elif "), self.expr(cond), text(":")]);
                    out = concat(
                        out,
                        concat(hardline(), self.suite(h, body, clause_start(i + 1))),
                    );
                }
                if let Some(eb) = else_body {
                    let e = self.suite(text("else:"), eb, region_end);
                    out = concat(out, concat(hardline(), e));
                }
                out
            }
            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                let head = concat_all([text("while "), self.expr(condition), text(":")]);
                let out = self.suite(head, body, self.else_bound(else_body, region_end));
                self.with_else(out, else_body, region_end)
            }
            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                let head = concat_all([
                    text("for "),
                    self.for_target(target),
                    text(" in "),
                    self.expr(iter),
                    text(":"),
                ]);
                let out = self.suite(head, body, self.else_bound(else_body, region_end));
                self.with_else(out, else_body, region_end)
            }
            StmtKind::With { items, body } => {
                let parts: Vec<Doc> = items
                    .iter()
                    .map(|it| {
                        let ctx = self.expr(&it.context_expr);
                        match &it.alias {
                            Some(a) => concat_all([ctx, text(" as "), self.expr(a)]),
                            None => ctx,
                        }
                    })
                    .collect();
                let head = concat_all([text("with "), join(text(", "), parts), text(":")]);
                self.suite(head, body, bound)
            }
            StmtKind::Return(v) => match v {
                Some(e) => concat(text("return "), self.expr(e)),
                None => text("return"),
            },
            StmtKind::Assert { test, msg } => {
                let mut d = concat(text("assert "), self.expr(test));
                if let Some(m) = msg {
                    d = concat_all([d, text(", "), self.expr(m)]);
                }
                d
            }
            StmtKind::Import { module, alias } => {
                let mut d = concat(text("import "), text(module.join(".")));
                if let Some(a) = alias {
                    d = concat_all([d, text(" as "), text(a.clone())]);
                }
                d
            }
            StmtKind::FromImport {
                module,
                names,
                is_star,
            } => {
                let head = concat_all([text("from "), text(module.join(".")), text(" import ")]);
                if *is_star {
                    return concat(head, text("*"));
                }
                let parts: Vec<Doc> = names
                    .iter()
                    .map(|(n, a)| match a {
                        Some(al) => text(format!("{n} as {al}")),
                        None => text(n.clone()),
                    })
                    .collect();
                concat(head, join(text(", "), parts))
            }
            StmtKind::NativeImport { .. } | StmtKind::PyImport { .. } => self.verbatim(s.span),
            StmtKind::Let {
                name,
                type_ann,
                value,
                is_mut,
                ..
            } => self.binding("let", *is_mut, std::slice::from_ref(name), type_ann, value),
            StmtKind::MultiLet {
                names,
                type_ann,
                value,
                is_mut,
                ..
            } => self.binding("let", *is_mut, names, type_ann, value),
            StmtKind::Const {
                name,
                type_ann,
                value,
                ..
            } => self.binding("const", false, std::slice::from_ref(name), type_ann, value),
            StmtKind::MultiConst {
                names,
                type_ann,
                value,
                ..
            } => self.binding("const", false, names, type_ann, value),
            StmtKind::Assign { target, value } => concat_all([
                self.maybe_bare_tuple(target),
                text(" = "),
                self.maybe_bare_tuple(value),
            ]),
            StmtKind::AugAssign { target, op, value } => concat_all([
                self.expr(target),
                text(format!(" {} ", augop_str(op))),
                self.expr(value),
            ]),
            StmtKind::Pass => text("pass"),
            StmtKind::Break => text("break"),
            StmtKind::Continue => text("continue"),
            StmtKind::Defer(e) => concat(text("defer "), self.expr(e)),
            StmtKind::UnsafeBlock(body) => self.suite(text("unsafe:"), body, bound),
            StmtKind::ExprStmt(e) => self.expr(e),
        }
    }

    fn binding(
        &mut self,
        kw: &str,
        is_mut: bool,
        names: &[String],
        type_ann: &Option<TypeExpr>,
        value: &Expr,
    ) -> Doc {
        let mut d = text(kw.to_string());
        d = concat(d, text(" "));
        if is_mut {
            d = concat(d, text("mut "));
        }
        d = concat(d, text(names.join(", ")));
        if let Some(t) = type_ann {
            d = concat_all([d, text(": "), text(t.to_string())]);
        }
        concat_all([d, text(" = "), self.maybe_bare_tuple(value)])
    }

    pub(super) fn for_target(&self, t: &ForTarget) -> Doc {
        match t {
            ForTarget::Name(n, _) => text(n.clone()),
            ForTarget::Tuple(names) => {
                let parts = names.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
                text(parts.join(", "))
            }
        }
    }

    /// Render an expression that may be a bare (parenthesis-free) tuple, as used on
    /// either side of an assignment or `let`.
    fn maybe_bare_tuple(&mut self, e: &Expr) -> Doc {
        if let ExprKind::Tuple(elems) = &e.kind
            && !self.span_starts_with(e.span, '(')
        {
            let parts: Vec<Doc> = elems.iter().map(|x| self.expr(x)).collect();
            return join(text(", "), parts);
        }
        self.expr(e)
    }

    /// Render `e` as an operand of a binary operator at precedence `parent`,
    /// inserting parentheses only when the AST's structure would otherwise be lost.
    /// `tie_breaks` is true on the associativity-sensitive side (right operand of a
    /// left-associative op, left operand of right-associative `**`).
    pub(super) fn child(&mut self, e: &Expr, parent: u8, tie_breaks: bool) -> Doc {
        let cp = expr_prec(e);
        let need = if tie_breaks {
            cp <= parent
        } else {
            cp < parent
        };
        let inner = self.expr(e);
        if need {
            concat_all([text("("), inner, text(")")])
        } else {
            inner
        }
    }
}
