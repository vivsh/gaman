mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use gaman::{Dialect, Executor, ExecutorError, Introspectable, PostgresExecutor, Schema};
use postgres::{Client, NoTls};

use common::harness::DbHarness;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn test_db_url() -> Option<String> {
    std::env::var("TEST_DATABASE_URL").ok()
}

// Each test gets its own schema: gaman_test_{n}
// PgHarness creates it on construction and drops it in reset() / Drop.
struct PgHarness {
    client: Client,
    schema: String,
}

impl PgHarness {
    fn new() -> Option<Self> {
        let url = test_db_url()?;
        let mut client = Client::connect(&url, NoTls).ok()?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let schema = format!("gaman_test_{n}");
        client
            .execute(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""), &[])
            .ok()?;
        client
            .execute(&format!("SET search_path TO \"{schema}\""), &[])
            .ok()?;
        Some(Self { client, schema })
    }

    fn drop_schema(&mut self) {
        let _ = self.client.execute(
            &format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema),
            &[],
        );
    }
}

impl Drop for PgHarness {
    fn drop(&mut self) {
        self.drop_schema();
    }
}

// PgHarness itself implements Executor so we don't need a temporary wrapper.
impl Executor for PgHarness {
    fn execute(&mut self, sql: &str) -> Result<(), ExecutorError> {
        self.client
            .execute(sql, &[])
            .map(|_| ())
            .map_err(|e| ExecutorError::Execute(format!("{e}\n  SQL: {sql}")))
    }

    fn fetch_strings(&mut self, sql: &str) -> Result<Vec<String>, ExecutorError> {
        let rows = self.client.query(sql, &[]).map_err(|e| ExecutorError::Fetch(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    fn begin(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("BEGIN", &[]).map(|_| ()).map_err(|e| ExecutorError::Transaction(e.to_string()))
    }

    fn commit(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("COMMIT", &[]).map(|_| ()).map_err(|e| ExecutorError::Transaction(e.to_string()))
    }

    fn rollback(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("ROLLBACK", &[]).map(|_| ()).map_err(|e| ExecutorError::Transaction(e.to_string()))
    }
}

impl DbHarness for PgHarness {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn executor(&mut self) -> &mut dyn Executor {
        self
    }

    fn reset(&mut self) {
        let schema = self.schema.clone();
        let _ = self.client.execute(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"), &[]);
        let _ = self.client.execute(&format!("CREATE SCHEMA \"{schema}\""), &[]);
        let _ = self.client.execute(&format!("SET search_path TO \"{schema}\""), &[]);
    }

    fn table_exists(&mut self, table: &str) -> bool {
        let schema = self.schema.clone();
        let row = self.client.query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = $1 AND table_name = $2
            )",
            &[&schema, &table],
        );
        row.map(|r| r.get::<_, bool>(0)).unwrap_or(false)
    }

    fn tracking_count(&mut self) -> usize {
        let row = self
            .client
            .query_one("SELECT COUNT(*) FROM gaman_migrations", &[]);
        match row {
            Ok(r) => r.get::<_, i64>(0) as usize,
            Err(_) => 0,
        }
    }

    fn current_schema(&self) -> String {
        self.schema.clone()
    }

    fn inspect_schema(&mut self, schema: &str) -> Option<Schema> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        let client = Client::connect(&url, NoTls).ok()?;
        let mut executor = PostgresExecutor::new(client);
        executor.inspect_db(&[schema]).ok()
    }

    fn run_verify(&mut self, m: &gaman::Migrator, schema: &str) -> Option<Vec<gaman::Operation>> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        let client = Client::connect(&url, NoTls).ok()?;
        let mut executor = PostgresExecutor::new(client);
        executor.execute(&format!("SET search_path TO \"{}\"", self.schema)).ok()?;
        m.verify(&mut executor, schema).ok()
    }
}

macro_rules! pg_test {
    ($name:ident) => {
        #[test]
        #[ignore = "set TEST_DATABASE_URL and pass -- --include-ignored to run"]
        fn $name() {
            let mut h = PgHarness::new()
                .expect("TEST_DATABASE_URL must be set to run Postgres integration tests");
            common::harness::$name(&mut h);
        }
    };
}

pg_test!(test_forward_apply);
pg_test!(test_rollback_to_target);
pg_test!(test_fake_apply);
pg_test!(test_fake_rollback);
pg_test!(test_bootstrap_idempotent);
pg_test!(test_partial_failure_rolls_back);
pg_test!(test_duplicate_record_skipped);
pg_test!(test_drifted_tracking_reapplied);
pg_test!(test_invalid_graph_rejected);
pg_test!(test_replay_matches_inspect_db);
pg_test!(test_verify_no_drift_after_migrate);
