//! Search orchestration: bilingual query expansion, multi-source research, and report formatting.

pub(crate) mod bilingual;
pub(crate) mod engine;
mod lang;

pub use lang::Lang;
