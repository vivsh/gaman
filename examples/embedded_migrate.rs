/// Embedded mode: connect and migrate on startup.
/// Useful when the app should apply bundled migrations and keep running.
///
///   cargo run --example embedded_migrate
use gaman::{Config, MigrationEngine, include_migrations};

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    let _ = dotenvy::dotenv();

    MigrationEngine::new(Config::default(), MIGRATIONS)
        .migrate()
        .expect("migrations failed");

    // start server, run jobs, etc.
}
