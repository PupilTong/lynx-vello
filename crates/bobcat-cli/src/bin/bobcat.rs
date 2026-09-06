fn main() {
    if let Err(error) = bobcat_cli::cli::run_from_env() {
        eprintln!("bobcat: {error}");
        if error.is_argument_error() {
            eprintln!();
            eprintln!("{}", bobcat_cli::cli::USAGE);
        }
        std::process::exit(error.exit_code());
    }
}
