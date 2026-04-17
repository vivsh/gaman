/// Embedded mode — apply migrations programmatically on startup.
///
/// No CLI, no arg parsing. Just connect and migrate.
/// Typical use: call this at the top of `main()` before starting a server.
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
