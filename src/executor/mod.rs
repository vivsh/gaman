use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("execute failed: {0}")]
    Execute(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("transaction error: {0}")]
    Transaction(String),
}

#[derive(Debug, Error)]
pub enum InvokerError {
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("no invoker provided for Invoke operation")]
    NoInvoker,
}

pub trait Executor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn fetch_strings<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>>;
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

pub trait Invoker {
    fn invoke(&self, command: &str, tx: &mut dyn Executor) -> Result<(), InvokerError>;
}

pub trait Introspectable {
    fn inspect_db<'a>(&'a mut self, schemas: &'a [&'a str]) -> BoxFuture<'a, Result<crate::states::Schema, ExecutorError>>;
}

pub mod postgres;
pub mod subprocess;
pub use postgres::PostgresExecutor;
pub use subprocess::SubprocessInvoker;
