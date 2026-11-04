use clap::Parser;

fn main() {
    let cli = crashtrace::cli::Cli::parse();
    if let Err(e) = crashtrace::cli::run(&cli) {
        eprintln!("crashtrace: {e:#}");
        std::process::exit(1);
    }
}
