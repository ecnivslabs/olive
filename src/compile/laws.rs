use crate::parser;
use crate::span;

pub(crate) const OLIVE_LAWS: &str = "The Laws of Olive, by Vince Swu\n\n\
No compromise.\n\
Readability is not optional.\n\
Complexity must justify itself.\n\
Power should not require ceremony.\n\
Safety should be free, not fought for.\n\
The obvious solution should be obvious.\n\
Simple things should be simple.\n\
Complex things should be possible.\n\
Purity must not outweigh practicality.\n\
What would Olive do?";

pub(crate) fn make_laws_stmt(span: span::Span) -> parser::Stmt {
    let callee = parser::Expr::new(parser::ExprKind::Identifier("print".to_string()), span);
    let arg = parser::Expr::new(parser::ExprKind::Str(OLIVE_LAWS.to_string()), span);
    let call = parser::Expr::new(
        parser::ExprKind::Call {
            callee: Box::new(callee),
            args: vec![parser::CallArg::Positional(arg)],
        },
        span,
    );
    parser::Stmt::new(parser::StmtKind::ExprStmt(call), span)
}

pub(crate) fn is_laws_import(module: &[String], alias: &Option<String>) -> bool {
    alias.is_none() && module.len() == 1 && module[0] == "olive"
}
