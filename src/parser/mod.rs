pub mod ast;
pub mod error;

use crate::lexer::Token;

pub struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
}

mod base;
mod decls;
mod expr;
mod ffi;
mod stmt;
mod tests;
mod types;

pub use ast::*;
