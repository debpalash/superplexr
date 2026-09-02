fn main() {
    if let Err(error) = termi9ne_server::run_from_env() {
        eprintln!("termi9ne daemon failed: {error}");
        std::process::exit(1);
    }
}
