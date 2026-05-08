use super::doc::*;
use super::lower::{Item, Lowerer};
use crate::parser::ast::*;

impl Lowerer<'_> {
    pub(super) fn lower_fn(&mut self, s: &Stmt, region_end: usize) -> Doc {
        let StmtKind::Fn {
            name,
            type_params,
            params,
            return_type,
            body,
            decorators,
            is_async,
        } = &s.kind
        else {
            unreachable!()
        };
        let mut head = self.decorators(decorators);
        if *is_async {
            head = concat(head, text("async "));
        }
        let param_docs: Vec<Doc> = params.iter().map(|p| self.param(p)).collect();
        head = concat_all([
            head,
            text("fn "),
            text(name.clone()),
            self.type_params(type_params),
            bracketed("(", param_docs, ")"),
        ]);
        if let Some(rt) = return_type {
            head = concat_all([head, text(" -> "), text(rt.to_string())]);
        }
        self.suite(concat(head, text(":")), body, region_end)
    }

    pub(super) fn lower_struct(&mut self, s: &Stmt, region_end: usize) -> Doc {
        let StmtKind::Struct {
            name,
            type_params,
            fields,
            body,
            decorators,
        } = &s.kind
        else {
            unreachable!()
        };
        let head = concat_all([
            self.decorators(decorators),
            text("struct "),
            text(name.clone()),
            self.type_params(type_params),
            text(":"),
        ]);
        if fields.is_empty() && body.is_empty() {
            return concat(head, nest(INDENT, concat(hardline(), text("pass"))));
        }
        let mut members: Vec<(usize, usize, Doc)> = Vec::new();
        for f in fields {
            members.push((f.span.start, f.span.end, self.param(f)));
        }
        let inner = self.weave_members(members, body, region_end);
        concat(head, nest(INDENT, concat(hardline(), inner)))
    }

    /// Struct bodies mix field declarations and method definitions; merge them in
    /// source order so comments and blank lines land correctly.
    fn weave_members(
        &mut self,
        fields: Vec<(usize, usize, Doc)>,
        body: &[Stmt],
        bound: usize,
    ) -> Doc {
        enum M<'b> {
            Field(usize, usize, Doc),
            Method(&'b Stmt),
        }
        let mut all: Vec<M> = Vec::new();
        for (st, en, d) in fields {
            all.push(M::Field(st, en, d));
        }
        for s in body {
            all.push(M::Method(s));
        }
        all.sort_by_key(|m| match m {
            M::Field(st, ..) => *st,
            M::Method(s) => s.span.start,
        });
        let starts: Vec<usize> = all
            .iter()
            .map(|m| match m {
                M::Field(st, ..) => *st,
                M::Method(s) => s.span.start,
            })
            .collect();
        let col = starts.first().map(|&s| self.cw.column_of(s)).unwrap_or(0);
        let mut items: Vec<Item> = Vec::new();
        for (i, m) in all.into_iter().enumerate() {
            let region_end = starts.get(i + 1).copied().unwrap_or(bound);
            let (st, en, doc, decl) = match m {
                M::Field(st, en, d) => (st, en, d, false),
                M::Method(s) => (s.span.start, s.span.end, self.stmt(s, region_end), true),
            };
            self.take_leading(&mut items, st);
            let sl = self.cw.line_of(st);
            let el = self.cw.line_of(en.saturating_sub(1));
            let mut doc = doc;
            if let Some(c) = self.cw.take_trailing(el, en) {
                doc = concat(doc, concat(text("  "), self.comment_doc(&c)));
            }
            items.push(Item::new(sl, el, decl, doc));
        }
        self.flush_block(&mut items, bound, col);
        Self::assemble(items)
    }

    pub(super) fn lower_enum(&mut self, s: &Stmt, region_end: usize) -> Doc {
        let StmtKind::Enum {
            name,
            type_params,
            variants,
            body,
            decorators,
        } = &s.kind
        else {
            unreachable!()
        };
        let head = concat_all([
            self.decorators(decorators),
            text("enum "),
            text(name.clone()),
            self.type_params(type_params),
            text(":"),
        ]);
        let mut parts: Vec<Doc> = Vec::new();
        for v in variants {
            let mut d = text(v.name.clone());
            if !v.types.is_empty() {
                let tys: Vec<Doc> = v.types.iter().map(|t| text(t.to_string())).collect();
                d = concat(d, bracketed("(", tys, ")"));
            }
            if let Some(val) = &v.value {
                d = concat_all([d, text(" = "), self.expr(val)]);
            }
            parts.push(d);
        }
        let mut inner = join(hardline(), parts);
        if !body.is_empty() {
            let methods = self.weave_stmts(body, region_end, false);
            inner = concat(inner, concat(hardline(), methods));
        }
        concat(head, nest(INDENT, concat(hardline(), inner)))
    }

    pub(super) fn param(&mut self, p: &Param) -> Doc {
        let mut d = match p.kind {
            ParamKind::KwArg => text("**"),
            ParamKind::VarArg => text("*"),
            ParamKind::Regular => nil(),
        };
        if p.is_mut {
            d = concat(d, text("mut "));
        }
        d = concat(d, text(p.name.clone()));
        if let Some(t) = &p.type_ann {
            d = concat_all([d, text(": "), text(t.to_string())]);
        }
        if let Some(def) = &p.default {
            d = concat_all([d, text(" = "), self.expr(def)]);
        }
        d
    }

    pub(super) fn type_params(&self, tps: &[String]) -> Doc {
        if tps.is_empty() {
            nil()
        } else {
            text(format!("[{}]", tps.join(", ")))
        }
    }

    pub(super) fn decorators(&self, decs: &[Decorator]) -> Doc {
        if decs.is_empty() {
            return nil();
        }
        let mut out = Doc::Nil;
        let mut i = 0;
        while i < decs.len() {
            if decs[i].is_directive {
                let mut names = Vec::new();
                while i < decs.len() && decs[i].is_directive {
                    names.push(decs[i].name.clone());
                    i += 1;
                }
                out = concat(out, text(format!("#[{}]", names.join(", "))));
            } else {
                out = concat(out, text(format!("@{}", decs[i].name)));
                i += 1;
            }
            out = concat(out, hardline());
        }
        out
    }
}
