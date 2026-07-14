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

/// Maximum statement/expression nesting the semantic phases walk. The parser's
/// own guard (`parser::MAX_NESTING_DEPTH`) is higher than what these phases can
/// survive: their per-level frames are far larger, and past ~140 nested levels
/// on an 8 MiB stack the process aborts before any diagnostic renders. 100
/// stays clear of that while sitting an order of magnitude above real code.
pub(crate) const MAX_SEMANTIC_NESTING: usize = 100;

pub use error::SemanticError;
pub use resolver::Resolver;
pub use type_checker::TypeChecker;
