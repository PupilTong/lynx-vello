fn main() {
    if let Err(error) = bobcat::cli::run_from_env() {
        eprintln!("bobcat: {error}");
        if error.is_argument_error() {
            eprintln!();
            eprintln!("{}", bobcat::cli::USAGE);
        }
        std::process::exit(error.exit_code());
    }
}
