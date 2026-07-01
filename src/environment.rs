use std::sync::Arc;

use thiserror::Error;

use crate::conf::Config;
use crate::dialects::Dialect;
use crate::executor::{BoxFuture, Executor, Introspectable};

#[derive(Debug, Error)]
pub enum EnvironmentError {
    #[error("{0}")]
    Config(String),
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

pub trait EnvironmentExecutor: Executor + Introspectable {}

impl<T> EnvironmentExecutor for T where T: Executor + Introspectable {}

pub trait Environment {
    fn config(&self) -> &Arc<Config>;
    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, EnvironmentError>>;

    fn dialect(&self) -> Dialect {
        self.config().dialect().unwrap_or(Dialect::Postgres)
    }
}
