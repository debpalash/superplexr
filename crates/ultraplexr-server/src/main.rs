fn main() {
    if let Err(error) = ultraplexr_server::run_from_env() {
        eprintln!("ultraplexr daemon failed: {error}");
        std::process::exit(1);
    }
}
