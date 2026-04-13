use thiserror::Error;

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
    fn execute(&mut self, sql: &str) -> Result<(), ExecutorError>;
    fn fetch_strings(&mut self, sql: &str) -> Result<Vec<String>, ExecutorError>;
    fn begin(&mut self) -> Result<(), ExecutorError>;
    fn commit(&mut self) -> Result<(), ExecutorError>;
    fn rollback(&mut self) -> Result<(), ExecutorError>;
    fn acquire_lock(&mut self) -> Result<(), ExecutorError> { Ok(()) }
    fn release_lock(&mut self) -> Result<(), ExecutorError> { Ok(()) }
}

pub trait Invoker {
    fn invoke(&self, command: &str, tx: &mut dyn Executor) -> Result<(), InvokerError>;
}

pub trait Introspectable {
    fn inspect_db(&mut self, schemas: &[&str]) -> Result<crate::states::SchemaState, ExecutorError>;
}

pub mod postgres;
pub mod subprocess;
pub use postgres::PostgresExecutor;
pub use subprocess::SubprocessInvoker;
