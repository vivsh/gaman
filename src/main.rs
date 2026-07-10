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

fn print_version() {
    println!("gaman {VERSION}");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() < 2 {
        print_header();
        std::process::exit(1);
    }
    if raw_args
        .iter()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        print_version();
        return;
    }
    if raw_args
        .iter()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "help")
    {
        println!("gaman {VERSION}");
        println!();
    }

    let args: GamanArgs = argh::from_env();
    let verbose = args.verbose || gaman_debug_enabled();

    if let Err(e) = handle_cmd(args).await {
        e.print(verbose);
        std::process::exit(1);
    }
}

fn gaman_debug_enabled() -> bool {
    std::env::var("GAMAN_DEBUG")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}
