/// Embedded mode with a schema.yaml file on disk.
/// Useful when migrations are bundled but the schema file stays editable.
///
///   cargo run --example embedded_yaml -- make_migration add_users
///   cargo run --example embedded_yaml -- migrate
///   cargo run --example embedded_yaml -- show_migrations
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};
use gaman::schema::Schema;

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn main() {
    let _ = dotenvy::dotenv();

    if let Err(e) = MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|_| Schema::load(std::path::Path::new("schema.yaml"))
            .expect("failed to load schema.yaml"))
        .handle_args()
    {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
