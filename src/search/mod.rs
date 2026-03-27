//! Search orchestration: bilingual query expansion, multi-source research, and report formatting.

pub(crate) mod bilingual;
pub(crate) mod engine;
mod lang;
pub(crate) mod topical;
pub(crate) mod url;

pub use lang::Lang;
