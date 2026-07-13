use gaman::cli::{GamanArgs, handle_cmd};
use gaman_core::command_args::{BuildInfo, product_presentation};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_header() {
    eprintln!(
        "{}",
        product_presentation().banner(BuildInfo {
            executable: "gaman",
            version: VERSION,
        })
    );
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
    if raw_args.len() == 2 && matches!(raw_args[1].as_str(), "--version" | "-V") {
        print_version();
        return;
    }
    if raw_args.len() == 2 && matches!(raw_args[1].as_str(), "--help" | "help") {
        println!("gaman {VERSION}");
        println!();
    }

    let args: GamanArgs = argh::from_env();
    if args.version {
        print_version();
        return;
    }
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
