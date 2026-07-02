pub mod borrow_check;
pub mod codegen;
pub mod commands;
pub mod compile;
pub mod diagnostics;
pub mod fmt;
pub mod lexer;
pub mod mangle;
pub mod mir;
pub mod parser;
pub mod semantic;
pub mod span;
pub mod tooling;

#[cfg(test)]
pub mod olive_tests;
#[cfg(test)]
#[cfg(test)]
pub mod regression_tests;
#[cfg(test)]
pub mod speculation_tests;
#[cfg(test)]
pub mod test_utils;
