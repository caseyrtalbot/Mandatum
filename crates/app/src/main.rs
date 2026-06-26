fn main() {
    if let Err(error) = mandatum_app::run() {
        eprintln!("mandatum: {error}");
        std::process::exit(1);
    }
}
