//! Command-line parsing, validation, dispatch, diagnostics, and presentation.

mod args;
mod diagnostic;
mod dispatch;

pub use args::GamanArgs;
pub use diagnostic::CommandError;
pub use dispatch::handle_cmd;
