/// Embedded mode: connect and migrate on startup.
/// Useful when the app should apply bundled migrations and keep running.
///
///   cargo run --example embedded_migrate
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let _ = dotenvy::dotenv();

    MigrationEngine::new(Config::default(), &MIGRATIONS)
        .migrate().await
        .expect("migrations failed");

    // start server, run jobs, etc.
}
