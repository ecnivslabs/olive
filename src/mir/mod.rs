pub mod ir;
pub mod builder;
pub mod liveness;
pub mod optimizer;

pub use ir::*;
pub use builder::MirBuilder;
pub use liveness::Liveness;
pub use optimizer::Inliner;
