/// Embedded mode with an external schema.yaml file.
///
/// Schema is loaded from disk and passed via the builder callback.
/// Migrations are embedded at compile time via `include_migrations!`.
///
///   cargo run --example embedded_yaml -- make_migration add_users
///   cargo run --example embedded_yaml -- migrate
///   cargo run --example embedded_yaml -- show_migrations
use gaman::{Config, MigrationEngine, include_migrations};
use gaman::schema::Schema;

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    let _ = dotenvy::dotenv();

    if let Err(e) = MigrationEngine::new(Config::default(), MIGRATIONS)
        .with_schema(|_| Schema::load(std::path::Path::new("schema.yaml"))
            .expect("failed to load schema.yaml"))
        .handle_args()
    {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
