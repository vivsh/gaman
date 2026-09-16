//! Dialect-specific SQL for checked managed-row writes and ownership guards.

mod render;

pub use render::render;
pub(crate) use render::{literal, quote_ident, quote_qualified};
