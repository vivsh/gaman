//! Browser bindings for Gaman's host-neutral migration engine.
//!
//! Instances are main-thread-only. JavaScript callback values are wrapped so
//! they satisfy the core engine's `Send` contract without being accessed from a
//! Worker or another WASM thread.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use gaman_core::clarifier::{Clarification, Decision};
use gaman_core::command_args::{Command as ArgsCommand, CommandArgs};
use gaman_core::{
    ApplyCommand, BoxFuture, COMMAND_PROTOCOL_VERSION, Command, CommandEnvelope, CommandError,
    CommandResponse, CommandResult, Dialect, Executor, ExecutorError, InspectionError, MakeCommand,
    Migration, MigrationRunner as CoreMigrationRunner, MigrationStore, SchemaInspector, StoreError,
    TrackingStore,
};
use js_sys::{Function, Promise, Reflect};
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

type CoreRunner = CoreMigrationRunner<JsMigrationStore, JsTrackingStore, JsExecutor>;

#[wasm_bindgen]
pub struct Schema {
    inner: gaman_core::schema::Schema,
}

#[wasm_bindgen]
impl Schema {
    #[wasm_bindgen(js_name = fromSql)]
    pub fn from_sql(source: &str, dialect: &str) -> Result<Schema, JsValue> {
        Ok(Self {
            inner: gaman_core::schema::Schema::from_sql_str(source, parse_dialect(dialect)?)
                .map_err(js_error)?,
        })
    }

    #[wasm_bindgen(js_name = fromYaml)]
    pub fn from_yaml(source: &str, dialect: &str) -> Result<Schema, JsValue> {
        Ok(Self {
            inner: gaman_core::schema::Schema::from_yaml_str(source, parse_dialect(dialect)?)
                .map_err(js_error)?,
        })
    }

    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(source: &str, dialect: &str) -> Result<Schema, JsValue> {
        Ok(Self {
            inner: gaman_core::schema::Schema::from_json_str(source, parse_dialect(dialect)?)
                .map_err(js_error)?,
        })
    }
}

/// Browser migration facade. `callbacks` may provide nested `migrations`,
/// `tracking`, and `executor` objects whose methods return values or Promises.
#[wasm_bindgen]
pub struct MigrationRunner {
    runner: CoreRunner,
    dialect: Dialect,
    schema: Option<gaman_core::schema::Schema>,
}

#[wasm_bindgen]
impl MigrationRunner {
    #[wasm_bindgen(constructor)]
    pub fn new(dialect: &str, callbacks: JsValue) -> Result<MigrationRunner, JsValue> {
        let dialect = parse_dialect(dialect)?;
        let migrations = JsMigrationStore::new(
            callback(&callbacks, "migrations", "load"),
            callback(&callbacks, "migrations", "save"),
        );
        let tracking = JsTrackingStore::new(
            callback(&callbacks, "tracking", "install"),
            callback(&callbacks, "tracking", "appliedIds"),
            callback(&callbacks, "tracking", "record"),
            callback(&callbacks, "tracking", "unrecord"),
        );
        let executor = JsExecutor::new(
            callback(&callbacks, "executor", "execute"),
            callback(&callbacks, "executor", "begin"),
            callback(&callbacks, "executor", "commit"),
            callback(&callbacks, "executor", "rollback"),
            callback(&callbacks, "executor", "acquireLock"),
            callback(&callbacks, "executor", "releaseLock"),
        );
        Ok(Self {
            runner: CoreMigrationRunner::new(dialect, migrations, tracking, executor),
            dialect,
            schema: None,
        })
    }

    pub fn set_schema(&mut self, schema: &Schema) {
        self.schema = Some(schema.inner.clone());
    }

    /// Runs an exact token array through the shared argh grammar and typed runner protocol.
    #[wasm_bindgen(js_name = runTokens)]
    pub async fn run_tokens(
        &mut self,
        tokens: Vec<String>,
        decisions: JsValue,
    ) -> Result<JsValue, JsValue> {
        let command = self.resolve_tokens(&tokens, decisions)?;
        self.run_portable(&command).await
    }

    /// Runs one versioned structured command request without textual parsing.
    #[wasm_bindgen(js_name = runCommand)]
    pub async fn run_command_request(&mut self, request: JsValue) -> Result<JsValue, JsValue> {
        let envelope: CommandEnvelope =
            serde_wasm_bindgen::from_value(request).map_err(js_error)?;
        if envelope.protocol_version != COMMAND_PROTOCOL_VERSION {
            let error = CommandError::UnsupportedProtocolVersion {
                expected: COMMAND_PROTOCOL_VERSION,
                observed: envelope.protocol_version,
            };
            return Err(serde_wasm_bindgen::to_value(&error.failure())
                .unwrap_or_else(|_| JsValue::from_str(&error.to_string())));
        }
        self.run_portable(&envelope.command).await
    }

    /// Returns argh-generated help for the command hierarchy without duplicating help text.
    #[wasm_bindgen(js_name = commandHelp)]
    pub fn command_help(command: Option<String>) -> String {
        CommandArgs::command_help(&["gaman"], command.as_deref()).output
    }
}

impl MigrationRunner {
    /// Parses exact host tokens and resolves them against this WASM instance's schema state.
    fn resolve_tokens(&self, tokens: &[String], decisions: JsValue) -> Result<Command, JsValue> {
        let token_refs = tokens.iter().map(String::as_str).collect::<Vec<_>>();
        let token_refs = token_refs.strip_prefix(&["gaman"]).unwrap_or(&token_refs);
        let args = match CommandArgs::parse(&["gaman"], token_refs) {
            Ok(args) => args,
            Err(diagnostic) if diagnostic.success => {
                return Err(serde_wasm_bindgen::to_value(&lines(help_lines()))
                    .unwrap_or_else(|_| JsValue::from_str("failed to serialize command help")));
            }
            Err(diagnostic) => return Err(js_error(diagnostic)),
        };
        self.resolve_command(args.command, decisions)
    }

    /// Executes one resolved core command for Rust hosts that already own typed schema state.
    pub async fn run_command(&mut self, command: &Command) -> Result<CommandResult, CommandError> {
        self.runner.run_command(command).await
    }

    /// Serializes one typed runner response or structured command failure for JavaScript hosts.
    async fn run_portable(&mut self, command: &Command) -> Result<JsValue, JsValue> {
        match self.run_command(command).await {
            Ok(result) => {
                serde_wasm_bindgen::to_value(&CommandResponse::new(result)).map_err(js_error)
            }
            Err(error) => Err(serde_wasm_bindgen::to_value(&error.failure())
                .unwrap_or_else(|_| JsValue::from_str(&error.to_string()))),
        }
    }

    /// Resolves parsed browser command arguments into one core runner command.
    fn resolve_command(
        &self,
        command: ArgsCommand,
        decisions: JsValue,
    ) -> Result<Command, JsValue> {
        let decisions = parse_decisions(decisions)?;
        match command {
            ArgsCommand::Make(command) if command.empty => command
                .name
                .map(|name| Command::Make(MakeCommand::Empty { name }))
                .ok_or_else(|| JsValue::from_str("--empty requires a migration name")),
            ArgsCommand::Make(command) if command.merge => command
                .name
                .map(|name| Command::Make(MakeCommand::Merge { name }))
                .ok_or_else(|| JsValue::from_str("--merge requires a migration name")),
            ArgsCommand::Make(command) => {
                let schema = self
                    .schema
                    .clone()
                    .ok_or_else(|| JsValue::from_str("set a schema before running make"))?;
                Ok(Command::Make(if command.check {
                    MakeCommand::Check { schema, decisions }
                } else {
                    MakeCommand::Generate {
                        schema,
                        name: command.name,
                        dry_run: command.dry_run,
                        decisions,
                    }
                }))
            }
            ArgsCommand::Apply(command) => Ok(Command::Apply(if command.plan {
                ApplyCommand::Plan
            } else if command.check {
                ApplyCommand::Check
            } else {
                ApplyCommand::Execute {
                    target: command.target,
                    fake: command.fake,
                    fake_verified: command.fake_verified,
                    schemas: command.schema,
                }
            })),
            ArgsCommand::Status(command) => Ok(Command::Status {
                reverse: command.reverse,
                search: command.search,
            }),
            ArgsCommand::Show(command) => Ok(Command::Show {
                id: command.id,
                reverse: command.reverse,
                search: command.search,
            }),
            ArgsCommand::Sql(command) => Ok(Command::Sql {
                id: command.id,
                backwards: command.backwards,
            }),
            ArgsCommand::Config(_) => Err(JsValue::from_str(&format!(
                "config is host-specific; current dialect is {}",
                self.dialect.as_str()
            ))),
            _ => Err(JsValue::from_str(
                "this WASM host does not provide schema inspection or SQL file resolution",
            )),
        }
    }
}

/// Parses optional serialized clarification decisions supplied by the browser host.
fn parse_decisions(decisions: JsValue) -> Result<Vec<Decision>, JsValue> {
    if decisions.is_undefined() || decisions.is_null() {
        Ok(Vec::new())
    } else {
        serde_wasm_bindgen::from_value(decisions).map_err(js_error)
    }
}

#[derive(Serialize)]
struct Output {
    lines: Vec<String>,
    clarifications: Vec<Clarification>,
}
fn lines(lines: Vec<String>) -> Output {
    Output {
        lines,
        clarifications: Vec::new(),
    }
}

fn help_lines() -> Vec<String> {
    CommandArgs::command_help(&["gaman"], None)
        .output
        .lines()
        .map(str::to_string)
        .collect()
}

#[derive(Clone)]
struct JsCallback(SendWrapper<Function>);

impl JsCallback {
    async fn call(&self, arguments: &[JsValue]) -> Result<JsValue, String> {
        let value = match arguments {
            [] => self.0.call0(&JsValue::NULL),
            [first] => self.0.call1(&JsValue::NULL, first),
            _ => return Err("callbacks accept at most one argument".into()),
        }
        .map_err(js_value_error)?;
        let future = SendJsFuture(SendWrapper::new(JsFuture::from(Promise::resolve(&value))));
        future.await.map_err(js_value_error)
    }
}

struct SendJsFuture(SendWrapper<JsFuture>);

impl Future for SendJsFuture {
    type Output = Result<JsValue, JsValue>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.0).poll(context)
    }
}

#[derive(Clone)]
struct JsMigrationStore {
    load: Option<JsCallback>,
    save: Option<JsCallback>,
    memory: Arc<Mutex<Vec<Migration>>>,
}

#[derive(Serialize, Deserialize)]
struct StoredMigration {
    id: String,
    content: String,
}
impl JsMigrationStore {
    fn new(load: Option<JsCallback>, save: Option<JsCallback>) -> Self {
        Self {
            load,
            save,
            memory: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
impl MigrationStore for JsMigrationStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
        Box::pin(async move {
            match &self.load {
                Some(callback) => callback
                    .call(&[])
                    .await
                    .map_err(StoreError::unavailable)
                    .and_then(|value| {
                        let records = serde_wasm_bindgen::from_value::<Vec<StoredMigration>>(value)
                            .map_err(|error| StoreError::unavailable(error.to_string()))?;
                        records
                            .into_iter()
                            .map(|record| {
                                let mut migration = Migration::from_yaml_str(&record.content)
                                    .map_err(|error| StoreError::unavailable(error.to_string()))?;
                                migration.id = record.id;
                                Ok(migration)
                            })
                            .collect()
                    }),
                None => self
                    .memory
                    .lock()
                    .map_err(|_| StoreError::unavailable("migration memory lock poisoned"))
                    .map(|items| items.clone()),
            }
        })
    }
    fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            if let Some(callback) = &self.save {
                let record = StoredMigration {
                    id: migration.id.clone(),
                    content: migration
                        .to_yaml_string()
                        .map_err(|error| StoreError::unavailable(error.to_string()))?,
                };
                callback
                    .call(&[serde_wasm_bindgen::to_value(&record)
                        .map_err(|error| StoreError::unavailable(error.to_string()))?])
                    .await
                    .map_err(StoreError::unavailable)?;
            } else {
                self.memory
                    .lock()
                    .map_err(|_| StoreError::unavailable("migration memory lock poisoned"))?
                    .push(migration.clone());
            }
            Ok(())
        })
    }
}

#[derive(Clone)]
struct JsTrackingStore {
    install: Option<JsCallback>,
    applied: Option<JsCallback>,
    record: Option<JsCallback>,
    unrecord: Option<JsCallback>,
    memory: Arc<Mutex<HashSet<String>>>,
}
impl JsTrackingStore {
    fn new(
        install: Option<JsCallback>,
        applied: Option<JsCallback>,
        record: Option<JsCallback>,
        unrecord: Option<JsCallback>,
    ) -> Self {
        Self {
            install,
            applied,
            record,
            unrecord,
            memory: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}
impl TrackingStore for JsTrackingStore {
    fn install<'a>(
        &'a self,
        _dialect: Dialect,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), gaman_core::TrackingError>> {
        Box::pin(async move {
            if let Some(callback) = &self.install {
                callback
                    .call(&[])
                    .await
                    .map_err(gaman_core::TrackingError::unavailable)?;
            }
            Ok(())
        })
    }
    fn applied_ids<'a>(
        &'a self,
        _: Dialect,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, gaman_core::TrackingError>> {
        Box::pin(async move {
            match &self.applied {
                Some(callback) => callback
                    .call(&[])
                    .await
                    .map_err(gaman_core::TrackingError::unavailable)
                    .and_then(|value| {
                        serde_wasm_bindgen::from_value(value).map_err(|error| {
                            gaman_core::TrackingError::unavailable(error.to_string())
                        })
                    }),
                None => self
                    .memory
                    .lock()
                    .map_err(|_| {
                        gaman_core::TrackingError::unavailable("tracking memory lock poisoned")
                    })
                    .map(|ids| ids.clone()),
            }
        })
    }
    fn record<'a>(
        &'a self,
        _: Dialect,
        id: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), gaman_core::TrackingError>> {
        Box::pin(async move {
            if let Some(callback) = &self.record {
                callback
                    .call(&[JsValue::from_str(id)])
                    .await
                    .map_err(gaman_core::TrackingError::unavailable)?;
            } else {
                self.memory
                    .lock()
                    .map_err(|_| {
                        gaman_core::TrackingError::unavailable("tracking memory lock poisoned")
                    })?
                    .insert(id.into());
            }
            Ok(())
        })
    }
    fn unrecord<'a>(
        &'a self,
        _: Dialect,
        id: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), gaman_core::TrackingError>> {
        Box::pin(async move {
            if let Some(callback) = &self.unrecord {
                callback
                    .call(&[JsValue::from_str(id)])
                    .await
                    .map_err(gaman_core::TrackingError::unavailable)?;
            } else {
                self.memory
                    .lock()
                    .map_err(|_| {
                        gaman_core::TrackingError::unavailable("tracking memory lock poisoned")
                    })?
                    .remove(id);
            }
            Ok(())
        })
    }
}

struct JsExecutor {
    execute: Option<JsCallback>,
    begin: Option<JsCallback>,
    commit: Option<JsCallback>,
    rollback: Option<JsCallback>,
    acquire: Option<JsCallback>,
    release: Option<JsCallback>,
}
impl JsExecutor {
    fn new(
        execute: Option<JsCallback>,
        begin: Option<JsCallback>,
        commit: Option<JsCallback>,
        rollback: Option<JsCallback>,
        acquire: Option<JsCallback>,
        release: Option<JsCallback>,
    ) -> Self {
        Self {
            execute,
            begin,
            commit,
            rollback,
            acquire,
            release,
        }
    }
}
impl Executor for JsExecutor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.execute.as_ref(), Some(sql), "execute")
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.begin.as_ref(), None, "begin")
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.commit.as_ref(), None, "commit")
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.rollback.as_ref(), None, "rollback")
    }
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.acquire.as_ref(), None, "acquire lock")
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        invoke(self.release.as_ref(), None, "release lock")
    }
}

fn invoke<'a>(
    callback: Option<&'a JsCallback>,
    value: Option<&'a str>,
    operation: &'static str,
) -> BoxFuture<'a, Result<(), ExecutorError>> {
    Box::pin(async move {
        if let Some(callback) = callback {
            let args = value.map(JsValue::from_str);
            callback
                .call(args.as_slice())
                .await
                .map_err(|error| ExecutorError::Execute(format!("{operation}: {error}")))?;
        }
        Ok(())
    })
}

fn callback(callbacks: &JsValue, group: &str, name: &str) -> Option<JsCallback> {
    let group = Reflect::get(callbacks, &JsValue::from_str(group)).ok()?;
    let value = Reflect::get(&group, &JsValue::from_str(name)).ok()?;
    value
        .dyn_into::<Function>()
        .ok()
        .map(|function| JsCallback(SendWrapper::new(function)))
}

fn parse_dialect(input: &str) -> Result<Dialect, JsValue> {
    match input {
        "postgres" | "postgresql" => Ok(Dialect::Postgres),
        "sqlite" => Ok(Dialect::Sqlite),
        "mysql" => Ok(Dialect::Mysql),
        "mariadb" => Ok(Dialect::Mariadb),
        _ => Err(JsValue::from_str("unsupported dialect")),
    }
}
fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

impl SchemaInspector for JsExecutor {
    fn inspect<'a>(
        &'a mut self,
        _schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::schema::Schema, InspectionError>> {
        Box::pin(async {
            Err(InspectionError::query(
                "this WASM host does not provide live database inspection".to_string(),
            ))
        })
    }
}
fn js_value_error(value: JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| "JavaScript callback failed".into())
}
