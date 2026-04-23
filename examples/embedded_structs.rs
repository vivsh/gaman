/// Embedded mode with schema defined in Rust structs.
/// Use `#[derive(IntoTable)]` and pass the built schema into `with_schema`.
///
///   cargo run --example embedded_structs -- make_migration add_users
///   cargo run --example embedded_structs -- migrate
use gaman::{Config, EmbeddedMigrations, IntoTable, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

#[allow(dead_code)]
#[derive(IntoTable)]
struct User {
    id: i64,
    name: String,
    email: String,
    is_active: bool,
    #[column(nullable)]
    last_login: Option<String>,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(name = "posts")]
struct Post {
    id: i64,
    #[column(references = "users.id")]
    user_id: i64,
    title: String,
    body: String,
    #[column(default = "now()")]
    created_at: Option<String>,
}

fn main() {
    let _ = dotenvy::dotenv();

    let result = MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|s| s.table::<User>().table::<Post>().build())
        .and_then(|e| e.handle_args());

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
