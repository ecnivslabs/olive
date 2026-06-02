pub mod abi;
pub mod closure_check;
pub mod desugar;
mod error;
pub mod free_vars;
pub mod lint;
pub(crate) mod pyi;
mod resolver;
pub(crate) mod suggest;
mod symbol_table;
pub mod type_checker;
pub(crate) mod type_descriptor;
pub mod types;

#[cfg(test)]
mod tests_extended;

pub use error::SemanticError;
pub use resolver::Resolver;
pub use type_checker::TypeChecker;
