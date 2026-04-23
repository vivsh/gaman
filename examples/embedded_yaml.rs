/// Embedded mode with a schema.yaml file on disk.
/// Useful when migrations are bundled but the schema file stays editable.
///
///   cargo run --example embedded_yaml -- make_migration add_users
///   cargo run --example embedded_yaml -- migrate
///   cargo run --example embedded_yaml -- show_migrations
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let _ = dotenvy::dotenv();

    let result = MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|s| s.load_file("schema.yaml"))
        .and_then(|e| Ok(e.handle_args()));
    match result {
        Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
        Ok(fut) => if let Err(e) = fut.await { eprintln!("error: {e}"); std::process::exit(1); }
    }
}
