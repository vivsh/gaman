use gaman::cli::{GamanArgs, handle_cmd};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_header() {
    eprintln!();
    eprintln!("  Gaman v{VERSION}");
    eprintln!("  PostgreSQL-first, offline migration tool");
    eprintln!();
    eprintln!("  Type 'gaman --help' for usage.");
    eprintln!("  Type 'gaman <command> --help' for help on a specific command.");
    eprintln!();
}

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() < 2 {
        print_header();
        std::process::exit(1);
    }

    let _ = dotenvy::dotenv();

    let args: GamanArgs = argh::from_env();

    if let Err(e) = handle_cmd(args) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
