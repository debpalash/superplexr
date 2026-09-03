fn main() {
    if let Err(error) = superplexr_server::run_from_env() {
        eprintln!("superplexr daemon failed: {error}");
        std::process::exit(1);
    }
}
