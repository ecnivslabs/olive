pub mod ast;
pub mod error;

use crate::lexer::Token;

/// Maximum expression/type/statement nesting depth before parsing is rejected.
/// Bounds recursive-descent stack use so adversarial input (`((((...1...))))`,
/// 500-deep `if` pyramids) fails with E0200 instead of overflowing the native
/// stack and aborting the process. Sized to the *debug* build, whose frames are
/// several times fatter than release's: each paren level costs ~12 nested
/// parser calls, and past ~144 levels on an 8 MiB stack debug builds abort.
/// The semantic phases carry their own tighter guard (`MAX_SEMANTIC_NESTING`).
pub(crate) const MAX_NESTING_DEPTH: usize = 100;

pub struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
    pub(crate) depth: usize,
}

mod base;
mod decls;
mod expr;
mod ffi;
mod proptests;
mod stmt;
mod tests;
mod tests_extended;
mod types;

pub use ast::*;

#[cfg(test)]
mod mod_tests {
    use super::*;

    #[test]
    fn parser_struct_accessible() {
        let tokens = crate::lexer::Lexer::new("", 0).tokenise().unwrap();
        let p = Parser {
            tokens,
            pos: 0,
            depth: 0,
        };
        assert_eq!(p.pos, 0);
    }

    #[test]
    fn ast_types_accessible() {
        let _stmt = StmtKind::Pass;
        let _expr = ExprKind::Integer(42);
        let _ty = TypeExprKind::Name("int".into());
    }

    #[test]
    fn error_types_accessible() {
        let _err = error::ParseError {
            message: "test".into(),
            line: 1,
            col: 1,
            start: 0,
            end: 0,
        };
    }

    #[test]
    fn enums_have_expected_variants() {
        assert!(matches!(BinOp::Add, BinOp::Add));
        assert!(matches!(UnaryOp::Neg, UnaryOp::Neg));
        assert!(matches!(AugOp::Add, AugOp::Add));
        assert!(matches!(ParamKind::Regular, ParamKind::Regular));
    }

    #[test]
    fn program_can_be_constructed() {
        let s = Stmt::new(StmtKind::Pass, crate::span::Span::default());
        let prog = Program { stmts: vec![s] };
        assert_eq!(prog.stmts.len(), 1);
    }

    #[test]
    fn for_target_variants() {
        let sp = crate::span::Span::default();
        let _name = ForTarget::Name("x".into(), sp);
        let _tuple = ForTarget::Tuple(vec![("x".into(), sp), ("y".into(), sp)]);
    }

    #[test]
    fn match_pattern_variants() {
        let _wild = MatchPattern::Wildcard;
        let _lit = MatchPattern::Literal(Expr::new(
            ExprKind::Integer(1),
            crate::span::Span::default(),
        ));
        let _id = MatchPattern::Identifier("x".into(), crate::span::Span::default());
        let _var = MatchPattern::Variant("Some".into(), vec![]);
    }

    #[test]
    fn call_arg_variants() {
        let e = Expr::new(ExprKind::Integer(1), crate::span::Span::default());
        let _pos = CallArg::Positional(e.clone());
        let _kw = CallArg::Keyword("x".into(), e.clone());
        let _splat = CallArg::Splat(e.clone());
        let _kw_splat = CallArg::KwSplat(e);
    }
}
