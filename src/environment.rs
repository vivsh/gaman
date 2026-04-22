use std::sync::Arc;

use thiserror::Error;

use crate::conf::Config;
use crate::dialects::Dialect;
use crate::executor::{Executor, Introspectable, Invoker};

#[derive(Debug, Error)]
pub enum EnvironmentError {
    #[error("{0}")]
    Config(String),
    #[error("database connection failed: {0}")]
    Connect(String),
}

pub trait EnvironmentExecutor: Executor + Introspectable {}

impl<T> EnvironmentExecutor for T where T: Executor + Introspectable {}

pub trait Environment {
    fn config(&self) -> &Arc<Config>;
    fn executor(&self) -> Result<Box<dyn EnvironmentExecutor>, EnvironmentError>;
    fn invoker(&self) -> Result<Option<Box<dyn Invoker>>, EnvironmentError>;

    fn dialect(&self) -> Dialect {
        self.config().dialect().unwrap_or(Dialect::Postgres)
    }
}