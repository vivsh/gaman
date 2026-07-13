use gaman_core::{BoxFuture, Executor, ExecutorError, InspectionError, SchemaInspector};

use crate::conf::Config;
use crate::environment::EnvironmentExecutor;
use crate::executor::connect_environment_executor;
/// Defers a native database connection until one command actually needs it.
pub struct LazyExecutor {
    config: Config,
    inner: Option<Box<dyn EnvironmentExecutor + Send>>,
}

impl LazyExecutor {
    /// Creates an unopened executor for one configured database target.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            inner: None,
        }
    }

    /// Opens and retains the configured database connection on first live use.
    async fn connection(
        &mut self,
    ) -> Result<&mut (dyn EnvironmentExecutor + Send + 'static), String> {
        if self.inner.is_none() {
            let connection = connect_environment_executor(
                self.config.dialect,
                &self.config.database_url,
                self.config.tls,
            )
            .await
            .map_err(|error| match error {
                crate::executor::ConnectError::Config(message)
                | crate::executor::ConnectError::Connect(message) => message,
            })?;
            self.inner = Some(connection);
        }
        self.inner
            .as_deref_mut()
            .ok_or_else(|| "database connection was not initialized".to_string())
    }
}

impl Executor for LazyExecutor {
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Prepare)?
                .prepare(sql)
                .await
        })
    }

    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Execute)?
                .execute(sql)
                .await
        })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Fetch)?
                .fetch_strings(sql)
                .await
        })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Transaction)?
                .begin()
                .await
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Transaction)?
                .commit()
                .await
        })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Transaction)?
                .rollback()
                .await
        })
    }

    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Transaction)?
                .acquire_lock()
                .await
        })
    }

    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(ExecutorError::Transaction)?
                .release_lock()
                .await
        })
    }
}

impl SchemaInspector for LazyExecutor {
    fn inspect<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::schema::Schema, InspectionError>> {
        Box::pin(async move {
            self.connection()
                .await
                .map_err(InspectionError::query)?
                .inspect(schemas)
                .await
        })
    }
}
