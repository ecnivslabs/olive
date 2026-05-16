use crate::mir::ir::MirFunction;
use crate::borrow_check::checker::BorrowChecker;
use crate::parser::Parser;
use crate::lexer::Lexer;
use crate::semantic::resolver::Resolver;
use crate::semantic::type_checker::TypeChecker;
use crate::mir::builder::MirBuilder;

#[test]
fn debug_borrow_check() {
    let src = "fn consume(xs: [i64]) -> i64:\n    return 0\n\nfn caller() -> i64:\n    let mut xs = [1, 2]\n    let r = &xs\n    consume(xs)\n    return 0\n";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenise().unwrap();
    let mut parser = Parser::new(tokens);
    let mut prog = parser.parse_program().unwrap();
    let mut resolver = Resolver::new();
    resolver.resolve_program(&mut prog);
    let mut type_checker = TypeChecker::new();
    type_checker.check_program(&prog);
    let mut builder = MirBuilder::new();
    builder.build_program(&prog);
    
    for func in &builder.functions {
        if func.name == "caller" {
            println!("Function: {}", func.name);
            for (i, local) in func.locals.iter().enumerate() {
                println!("  Local {}: name={:?}, ty={:?}", i, local.name, local.ty);
            }
            let mut bc = BorrowChecker::new(func);
            bc.check();
            for err in bc.errors {
                println!("  Error: {:?}", err);
            }
        }
    }
}
