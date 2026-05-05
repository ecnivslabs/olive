mod error;
pub(crate) mod pyi;
mod resolver;
mod symbol_table;
pub mod type_checker;
pub mod types;

#[cfg(test)]
mod tests_extended;

pub use error::SemanticError;
pub use resolver::Resolver;
pub use type_checker::TypeChecker;
