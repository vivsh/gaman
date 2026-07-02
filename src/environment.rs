use std::sync::Arc;

use thiserror::Error;

use crate::conf::Config;
use crate::executor::{BoxFuture, Executor, Introspectable};
use gaman_core::dialects::Dialect;

#[derive(Debug, Error)]
/// Errors returned while preparing a live database executor for migration work.
pub enum EnvironmentError {
    /// The runtime configuration is incomplete or incompatible with the enabled features.
    #[error("{0}")]
    Config(String),
    /// A configured database connection could not be opened.
    #[error("database connection failed: {0}")]
    Connect(String),
}

impl From<crate::executor::ConnectError> for EnvironmentError {
    fn from(value: crate::executor::ConnectError) -> Self {
        match value {
            crate::executor::ConnectError::Config(message) => Self::Config(message),
            crate::executor::ConnectError::Connect(message) => Self::Connect(message),
        }
    }
}

/// Object-safe bundle for live database work.
///
/// An environment executor combines statement execution with database
/// introspection so the live migrator can apply migrations, inspect schema
/// state, and verify drift through one backend object. Implementations must be
/// `Send` so migration futures can move across async runtime threads. The trait
/// is not limited to SQLx: it is also the integration point for future dialects
/// or database clients outside SQLx, and for mock executors used by lifecycle
/// tests.
pub trait EnvironmentExecutor: Executor + Introspectable + Send {}

impl<T> EnvironmentExecutor for T where T: Executor + Introspectable + Send {}

/// Native live-database boundary for migration application, inspect, and verify.
///
/// Offline planning, replay, diffing, SQL rendering, and browser/WASM use do
/// not require an environment. This abstraction exists so Gaman's live path can
/// obtain a configured executor without coupling the migrator to one database
/// client implementation or test double. Implementations must be `Send + Sync`
/// because live migration futures borrow the migrator across await points.
pub trait Environment: Send + Sync {
    /// Returns the runtime configuration used to connect and infer defaults.
    fn config(&self) -> &Arc<Config>;

    /// Opens a live executor for the configured database target.
    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>>;

    /// Returns the selected SQL dialect, defaulting to Postgres when absent.
    fn dialect(&self) -> Dialect {
        self.config().dialect().unwrap_or(Dialect::Postgres)
    }
}
