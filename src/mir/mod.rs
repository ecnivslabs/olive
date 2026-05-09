pub mod builder;
pub mod ir;
pub mod liveness;
pub mod optimizer;

pub use builder::MirBuilder;
pub use ir::*;
pub use liveness::Liveness;
pub use optimizer::Inliner;
