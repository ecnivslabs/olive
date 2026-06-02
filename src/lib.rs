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
pub mod builtin_tests;
#[cfg(test)]
pub mod collection_method_tests;
pub mod eq_tests;
#[cfg(test)]
pub mod hash_tests;
#[cfg(test)]
pub mod narrow_tests;
#[cfg(test)]
pub mod numeric_underscore_tests;
#[cfg(test)]
pub mod olive_tests;
#[cfg(test)]
pub mod opt_attr_tests;
#[cfg(test)]
pub mod power_tests;
#[cfg(test)]
pub mod regression_tests;
#[cfg(test)]
pub mod repeat_tests;
#[cfg(test)]
pub mod scalar_attr_tests;
#[cfg(test)]
pub mod small_reflex_tests;
#[cfg(test)]
pub mod speculation_tests;
#[cfg(test)]
pub mod string_method_tests;
#[cfg(test)]
pub mod test_utils;
#[cfg(test)]
pub mod type_alias_tests;
